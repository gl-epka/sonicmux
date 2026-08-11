use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use ratatui::crossterm::event::{self, Event};
use tokio::sync::mpsc;

use crate::model::Msg;

pub(crate) struct InputReader {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl InputReader {
    pub(crate) fn spawn(sender: mpsc::Sender<Msg>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("sonicmux-tui-input".to_owned())
            .spawn(move || read_loop(&sender, &thread_stop))
            .ok();
        Self { stop, thread }
    }

    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ignored = thread.join();
        }
    }
}

impl Drop for InputReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ignored = thread.join();
        }
    }
}

fn read_loop(sender: &mpsc::Sender<Msg>, stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        match event::poll(Duration::from_millis(50)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) => {
                    if sender.blocking_send(Msg::Input(key)).is_err() {
                        break;
                    }
                }
                Ok(Event::Paste(value)) => {
                    if sender.blocking_send(Msg::Paste(value)).is_err() {
                        break;
                    }
                }
                Ok(Event::Resize(_, _)) => {
                    if sender.blocking_send(Msg::Resize).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(%error, "terminal input failed");
                    break;
                }
            },
            Ok(false) => {}
            Err(error) => {
                tracing::error!(%error, "terminal input polling failed");
                break;
            }
        }
    }
}
