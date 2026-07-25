use std::{io, time::Duration};

use async_trait::async_trait;
use tokio::time::MissedTickBehavior;

use crate::tui::components::Component;

use super::{TerminalBackend, TerminalEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiControl {
    Continue,
    Exit,
}

#[async_trait]
pub trait TuiApp: Component {
    async fn handle_terminal_input(&mut self, data: &str) -> TuiControl;

    fn handle_terminal_resize(&mut self, _columns: u16, _rows: u16) {}

    /// Applies state produced by background operations and returns whether the
    /// terminal needs to be redrawn.
    fn poll_background(&mut self) -> bool {
        false
    }
}

pub struct TuiRuntime<T> {
    terminal: T,
}

impl<T> TuiRuntime<T>
where
    T: TerminalBackend,
{
    pub fn new(terminal: T) -> Self {
        Self { terminal }
    }

    // Original:
    //   packages/pi-tui/src/tui.ts
    //   TUI.start(), TUI.handleInput(), TUI.requestRender()
    //
    // Rust adaptation:
    //   Crossterm's EventStream provides asynchronous input and resize events.
    //   The first interactive milestone performs a deterministic full redraw
    //   after every state transition. Differential redraw coalescing remains a
    //   documented follow-up and does not affect input routing correctness.
    pub async fn run<A>(&mut self, app: &mut A) -> io::Result<()>
    where
        A: TuiApp,
    {
        self.terminal.enter()?;
        let result = self.run_entered(app).await;
        let leave_result = self.terminal.leave();
        result.and(leave_result)
    }

    async fn run_entered<A>(&mut self, app: &mut A) -> io::Result<()>
    where
        A: TuiApp,
    {
        let (columns, rows) = self.terminal.size()?;
        app.handle_terminal_resize(columns, rows);
        self.render(app, columns)?;

        let mut background_tick = tokio::time::interval(Duration::from_millis(50));
        background_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        background_tick.tick().await;

        loop {
            tokio::select! {
                event = self.terminal.next_event() => {
                    let Some(event) = event? else {
                        break;
                    };
                    match event {
                        TerminalEvent::Input(data) => {
                            if app.handle_terminal_input(&data).await == TuiControl::Exit {
                                break;
                            }
                        }
                        TerminalEvent::Resize { columns, rows } => {
                            app.handle_terminal_resize(columns, rows);
                            app.invalidate();
                        }
                    }
                    let (columns, _) = self.terminal.size()?;
                    self.render(app, columns)?;
                }
                _ = background_tick.tick() => {
                    if app.poll_background() {
                        let (columns, _) = self.terminal.size()?;
                        self.render(app, columns)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn render(&mut self, app: &mut impl TuiApp, columns: u16) -> io::Result<()> {
        let lines = app.render(usize::from(columns).max(1));
        self.terminal.draw(&lines)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        any::Any,
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;

    use super::*;

    struct FakeTerminal {
        events: VecDeque<TerminalEvent>,
        size: (u16, u16),
        lifecycle: Arc<Mutex<Vec<&'static str>>>,
        frames: Arc<Mutex<Vec<Vec<String>>>>,
    }

    struct ChannelTerminal {
        events: tokio::sync::mpsc::UnboundedReceiver<TerminalEvent>,
        lifecycle: Arc<Mutex<Vec<&'static str>>>,
        frames: Arc<Mutex<Vec<Vec<String>>>>,
    }

    #[async_trait]
    impl TerminalBackend for ChannelTerminal {
        fn enter(&mut self) -> io::Result<()> {
            self.lifecycle.lock().expect("lifecycle").push("enter");
            Ok(())
        }

        fn leave(&mut self) -> io::Result<()> {
            self.lifecycle.lock().expect("lifecycle").push("leave");
            Ok(())
        }

        fn size(&self) -> io::Result<(u16, u16)> {
            Ok((40, 12))
        }

        fn draw(&mut self, lines: &[String]) -> io::Result<()> {
            self.frames.lock().expect("frames").push(lines.to_vec());
            Ok(())
        }

        async fn next_event(&mut self) -> io::Result<Option<TerminalEvent>> {
            Ok(self.events.recv().await)
        }
    }

    #[async_trait]
    impl TerminalBackend for FakeTerminal {
        fn enter(&mut self) -> io::Result<()> {
            self.lifecycle.lock().expect("lifecycle").push("enter");
            Ok(())
        }

        fn leave(&mut self) -> io::Result<()> {
            self.lifecycle.lock().expect("lifecycle").push("leave");
            Ok(())
        }

        fn size(&self) -> io::Result<(u16, u16)> {
            Ok(self.size)
        }

        fn draw(&mut self, lines: &[String]) -> io::Result<()> {
            self.frames.lock().expect("frames").push(lines.to_vec());
            Ok(())
        }

        async fn next_event(&mut self) -> io::Result<Option<TerminalEvent>> {
            Ok(self.events.pop_front())
        }
    }

    #[derive(Default)]
    struct TestApp {
        inputs: Vec<String>,
        size: (u16, u16),
    }

    impl Component for TestApp {
        fn render(&mut self, width: usize) -> Vec<String> {
            vec![format!("{width}:{}", self.inputs.join(","))]
        }

        fn invalidate(&mut self) {}

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[async_trait]
    impl TuiApp for TestApp {
        async fn handle_terminal_input(&mut self, data: &str) -> TuiControl {
            if data == "quit" {
                TuiControl::Exit
            } else {
                self.inputs.push(data.to_owned());
                TuiControl::Continue
            }
        }

        fn handle_terminal_resize(&mut self, columns: u16, rows: u16) {
            self.size = (columns, rows);
        }
    }

    struct BackgroundApp {
        pending_update: bool,
        applied: bool,
    }

    impl Component for BackgroundApp {
        fn render(&mut self, _width: usize) -> Vec<String> {
            vec![if self.applied { "applied" } else { "pending" }.to_owned()]
        }

        fn invalidate(&mut self) {}

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[async_trait]
    impl TuiApp for BackgroundApp {
        async fn handle_terminal_input(&mut self, _data: &str) -> TuiControl {
            TuiControl::Continue
        }

        fn poll_background(&mut self) -> bool {
            if !self.pending_update {
                return false;
            }
            self.pending_update = false;
            self.applied = true;
            true
        }
    }

    #[tokio::test]
    async fn enters_routes_input_renders_and_restores_terminal() {
        let lifecycle = Arc::new(Mutex::new(Vec::new()));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let terminal = FakeTerminal {
            events: VecDeque::from([
                TerminalEvent::Input("a".to_owned()),
                TerminalEvent::Resize {
                    columns: 40,
                    rows: 12,
                },
                TerminalEvent::Input("quit".to_owned()),
            ]),
            size: (40, 12),
            lifecycle: Arc::clone(&lifecycle),
            frames: Arc::clone(&frames),
        };
        let mut runtime = TuiRuntime::new(terminal);
        let mut app = TestApp::default();

        runtime.run(&mut app).await.expect("runtime");

        assert_eq!(app.inputs, ["a"]);
        assert_eq!(app.size, (40, 12));
        assert_eq!(*lifecycle.lock().expect("lifecycle"), ["enter", "leave"]);
        assert_eq!(
            *frames.lock().expect("frames"),
            [
                vec!["40:".to_owned()],
                vec!["40:a".to_owned()],
                vec!["40:a".to_owned()]
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn redraws_background_updates_while_waiting_for_terminal_input() {
        let lifecycle = Arc::new(Mutex::new(Vec::new()));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let (events_tx, events) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(75)).await;
            drop(events_tx);
        });
        let terminal = ChannelTerminal {
            events,
            lifecycle: Arc::clone(&lifecycle),
            frames: Arc::clone(&frames),
        };
        let mut runtime = TuiRuntime::new(terminal);
        let mut app = BackgroundApp {
            pending_update: true,
            applied: false,
        };

        runtime.run(&mut app).await.expect("runtime");

        assert_eq!(*lifecycle.lock().expect("lifecycle"), ["enter", "leave"]);
        assert_eq!(
            *frames.lock().expect("frames"),
            [vec!["pending".to_owned()], vec!["applied".to_owned()]]
        );
    }
}
