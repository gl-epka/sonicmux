import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, describe, expect, it } from "vitest";

import App from "./App.svelte";

afterEach(() => {
  cleanup();
  window.history.replaceState({}, "", "/");
});

describe("SonicMux desktop interface", () => {
  it("offers both native input routes in the empty state", async () => {
    window.history.replaceState({}, "", "/?demo=empty");
    render(App);

    expect(await screen.findByRole("heading", { name: "Drop MKV files here" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Choose MKV files" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Choose folder" })).toBeEnabled();
  });

  it("shows queue recovery and track details without relying on color", async () => {
    window.history.replaceState({}, "", "/?demo=planned");
    render(App);

    expect(await screen.findByText("Needs attention")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
    expect(screen.getByText("TrueHD Atmos")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start conversion" })).toBeEnabled();
  });

  it("exposes cancellation as the only primary batch action while running", async () => {
    window.history.replaceState({}, "", "/?demo=running");
    render(App);

    const cancel = await screen.findByRole("button", { name: "Cancel batch" });
    expect(cancel).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Start conversion" })).not.toBeInTheDocument();

    await fireEvent.click(cancel);
    await waitFor(() => expect(screen.getByRole("button", { name: "Start conversion" })).toBeInTheDocument());
  });

  it("presents explicit setup instead of downloading tools", async () => {
    window.history.replaceState({}, "", "/?demo=setup");
    render(App);

    expect(await screen.findByRole("heading", { name: "Connect FFmpeg to SonicMux" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Choose FFmpeg" })).toBeEnabled();
    expect(screen.getByText(/Nothing will be downloaded automatically/)).toBeInTheDocument();
  });

  it("windows large queues while retaining the full item count", async () => {
    window.history.replaceState({}, "", "/?demo=large");
    const view = render(App);

    expect(await screen.findByRole("heading", { name: "Conversion queue 120" })).toBeInTheDocument();
    await waitFor(() => expect(view.container.querySelectorAll(".queue-row").length).toBeLessThan(30));
  });
});
