# Dithus app — MVP Spec

## Problem Statement

Users want a real-time AI companion that sees their screen and hears them,
but existing options break on Sway with Wayland. Perssua falls into a
screen-share permission loop caused by live ScreenCast with an interactive
chooser, and Pluely fell short on model configuration. The need is a simple,
lightweight app that reads the screen, listens in real time, and answers in
a mini window.

## Solution

A native Rust app with an egui overlay: it captures the screen through
periodic still screenshots (no live ScreenCast, no chooser, no permission
loops), transcribes the microphone in real time, and shows the model answer
in a small always-on-top window. The model provider is configurable across
Ollama, OpenAI, and OpenCode models.

## User Stories

1. As a user, I want a small always-on-top window with the latest AI answer, so that I can read it without switching windows.
2. As a user, I want the app to capture my screen periodically, so that the AI sees what I see without me attaching anything.
3. As a user, I want an immediate capture via hotkey, so that I can ask about something that just appeared.
4. As a user, I want to speak and have my speech transcribed in real time, so that I can talk instead of typing.
5. As a user, I want to choose between Ollama, OpenAI, and OpenCode as provider, so that I can use local or cloud models depending on the day.
6. As a user, I want to switch models within a provider through simple config, so that I can balance cost and quality.
7. As a user, I want API keys kept out of the code via environment variables, so that credentials never leak.
8. As a user, I want to run fully local with Ollama, so that I can work with no account and no internet.
9. As a user, I want to use my OpenCode models, so that I benefit from subscriptions I already pay for.
10. As a user, I want the app to never open a share picker, so that the flow never stalls in a permission loop.
11. As a user, I want status (listening, thinking, error) in the mini window, so that I know what the app is doing.
12. As a user, I want a short visible conversation history, so that I can follow question and answer.
13. As a user, I want a single small binary, so that installation needs no heavy runtime.
14. As a user, I want capture interval and transcription language configurable, so that I control cost and accuracy.
15. As a user, I want clear logs when capture or transcription fails, so that I can diagnose without guessing.

## Implementation Decisions

- Stack: Rust with egui for the overlay (single binary, native Wayland via winit, no webview). Chosen over Fyne and Wails/WebKitGTK.
- Still-screenshot capture, never live ScreenCast. Uses the Screenshot portal over D-Bus, with fallback to `grim` on wlroots compositors. This decision comes directly from the Perssua bug investigation: interactive-chooser ScreenCast causes the loop; Screenshot opens no chooser.
- Audio: microphone only in v1, via PipeWire/Pulse. No system-audio capture in v1.
- Transcription: local whisper. Keeps the offline story coherent with Ollama usage.
- Single seam: a `ChatProvider` trait with three implementations, Ollama via localhost OpenAI-compatible API, OpenAI via HTTPS with environment key, OpenCode via local CLI in headless mode. Everything else only sees the trait.
- TOML config: active provider, model names, capture interval, language. Keys from environment only, never in the file.
- Overlay: small always-on-top window with state, latest answer, and short history. No stealth and no compositor fork (recorded decision: real share invisibility requires patching Sway, out of scope).
- Main loop on an interval: every N seconds it sends screenshot plus recent transcript to the provider and updates the answer.

## Testing Decisions

- Test external behavior, not internals: given fixture screenshot and audio, the pipeline must produce the expected provider call and update the UI.
- Modules under test: provider trait against a mock HTTP server (Ollama/OpenAI), capture fallback chain (portal unavailable falls back to `grim`), TOML parsing and environment key loading.
- No prior art in the repo: this is the first app project here, so the new tests set the standard.

## Out of Scope

- Share invisibility and any Sway patch or fork.
- Live ScreenCast and source pickers (snib, slurp, or similar).
- System-audio capture (PipeWire monitor).
- Global shortcuts via the Shortcuts portal.
- GNOME, KDE, Windows, or macOS support in v1 (the Screenshot portal is standard, but only Sway will be validated).
- AUR packaging and third-party docs (next phase).

## Further Notes

- Lessons from the Perssua investigation shaped this spec: bursts of portal-wlr `no output found` with Electron on XWayland, the app disabling PipeWire on X11, and a healthy standalone snib. The design avoids all three points.
- Open assumption: OpenCode access via headless CLI. If the CLI offers no suitable mode, fall back to its SDK or local API while keeping the trait intact.
