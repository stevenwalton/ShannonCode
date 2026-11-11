//! **Closed from outside**: the terminal going away (SIGHUP — a closed window,
//! a killed tmux pane) or a polite `kill` (SIGTERM).
//!
//! Their default action ends the process where it stands, with no unwinding,
//! so nothing was dropped and nothing was stopped. Every command Shannon starts
//! leads its own process group (`exec.rs`), which the terminal's hangup does
//! not reach, so a closed window left them all running. Caught here, either
//! signal becomes `:qa!`: every turn sealed, every command's group killed
//! (Steven, 2026-09-30: "ensure we are closing everything when we close
//! Shannon").

/// The signals that close Shannon from outside, listened for from launch.
pub(crate) struct ExitSignals {
    #[cfg(unix)]
    signals: [tokio::signal::unix::Signal; 2],
}

impl ExitSignals {
    /// Start listening. From here on, the two signals no longer end the
    /// process by themselves: [`recv`](Self::recv) must be waited on.
    pub(crate) fn listen() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                signals: [
                    signal(SignalKind::hangup())?,
                    signal(SignalKind::terminate())?,
                ],
            })
        }
        #[cfg(not(unix))]
        Ok(Self {})
    }

    /// The name of the signal that arrived, once one does; never, where
    /// there are none to wait for.
    pub(crate) async fn recv(&mut self) -> &'static str {
        #[cfg(unix)]
        {
            let [hangup, terminate] = &mut self.signals;
            tokio::select! {
                _ = hangup.recv() => "SIGHUP",
                _ = terminate.recv() => "SIGTERM",
            }
        }
        #[cfg(not(unix))]
        std::future::pending().await
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Send `signal` to this test process, as a closed terminal or a `kill`
    /// would. Safe only once a listener is installed: the default action for
    /// both ends the process, which is the bug this module exists for.
    fn signal_self(signal: &str) {
        let status = std::process::Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(std::process::id().to_string())
            .status()
            .expect("run kill");
        assert!(status.success(), "premise: kill -{signal} was delivered");
    }

    /// Both signals are caught and named. One test, not two: the handlers
    /// are process-wide, so two tests would each see the other's signal.
    #[tokio::test]
    async fn a_hangup_or_a_terminate_is_caught_and_named() {
        let mut signals = ExitSignals::listen().expect("listen");
        for name in ["HUP", "TERM"] {
            signal_self(name);
            let got = tokio::time::timeout(Duration::from_secs(5), signals.recv())
                .await
                .unwrap_or_else(|_| panic!("SIG{name} never arrived"));
            assert_eq!(got, format!("SIG{name}"));
        }
    }
}
