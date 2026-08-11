use color_eyre::eyre::{Result, WrapErr};

type Restore = Box<dyn FnOnce() -> Result<()> + Send>;

pub(crate) struct TerminalSession {
    terminal: ratatui::DefaultTerminal,
    restore: Option<Restore>,
}

impl TerminalSession {
    pub(crate) fn init() -> Result<Self> {
        let terminal = ratatui::try_init().wrap_err("failed to initialize the terminal")?;
        Ok(Self {
            terminal,
            restore: Some(Box::new(|| {
                ratatui::try_restore().wrap_err("failed to restore the terminal")
            })),
        })
    }

    pub(crate) const fn terminal(&mut self) -> &mut ratatui::DefaultTerminal {
        &mut self.terminal
    }

    pub(crate) fn finish<T>(mut self, result: Result<T>) -> Result<T> {
        let restore = self
            .restore
            .take()
            .map_or_else(|| Ok(()), |restore| restore());
        combine(result, restore)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if let Some(restore) = self.restore.take() {
            let _ignored = restore();
        }
    }
}

fn combine<T>(result: Result<T>, restore: Result<()>) -> Result<T> {
    match (result, restore) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(restore)) => Err(error.wrap_err(restore.to_string())),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::Restore;

    struct TestGuard(Option<Restore>);

    impl Drop for TestGuard {
        fn drop(&mut self) {
            if let Some(restore) = self.0.take() {
                let _ignored = restore();
            }
        }
    }

    #[test]
    fn restoration_callback_runs_during_unwind() {
        let restored = Arc::new(AtomicBool::new(false));
        let marker = Arc::clone(&restored);
        let outcome = std::panic::catch_unwind(move || {
            let _guard = TestGuard(Some(Box::new(move || {
                marker.store(true, Ordering::Release);
                Ok(())
            })));
            panic!("simulated TUI failure");
        });
        assert!(outcome.is_err());
        assert!(restored.load(Ordering::Acquire));
    }
}
