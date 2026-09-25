use std::io;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) enum DecoderWaitError {
    Cancelled,
    DeadlineExceeded,
    Io(io::Error),
}

// Audio EOF does not prove that the decoder has exited. Keep the same control
// boundary until the process is reaped, including when its pipes close early.
pub(crate) fn wait_for_decoder(
    child: &mut Child,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<ExitStatus, DecoderWaitError> {
    let result = loop {
        if cancelled.load(Ordering::Relaxed) {
            break Err(DecoderWaitError::Cancelled);
        }
        if Instant::now() >= deadline {
            break Err(DecoderWaitError::DeadlineExceeded);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => break Err(DecoderWaitError::Io(error)),
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
pub(crate) mod tests {
    use std::error::Error;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use tempfile::{TempDir, tempdir};

    // The normal test run returns immediately. A subprocess invocation stays alive
    // with its pipes open, without requiring a shell or an installed decoder.
    #[test]
    fn sleeping_decoder_fixture() -> Result<(), Box<dyn Error>> {
        let Some(ready) = std::env::var_os("MUSIC_ANALYSIS_SLEEPING_DECODER_READY_PATH") else {
            return Ok(());
        };
        std::fs::write(ready, b"ready")?;
        std::thread::sleep(Duration::from_secs(2));
        Ok(())
    }

    pub(crate) fn sleeping_decoder() -> Result<(TempDir, Child), Box<dyn Error>> {
        let directory = tempdir()?;
        let ready = directory.path().join("ready");
        let mut child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "decoder_process::tests::sleeping_decoder_fixture",
                "--nocapture",
            ])
            .env("MUSIC_ANALYSIS_SLEEPING_DECODER_READY_PATH", &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            if child.try_wait()?.is_some() || Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("decoder fixture did not become ready".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok((directory, child))
    }

    #[test]
    fn decoder_exit_wait_observes_deadline_and_reaps_the_child() -> Result<(), Box<dyn Error>> {
        let (_directory, mut child) = sleeping_decoder()?;
        let result = super::wait_for_decoder(
            &mut child,
            Instant::now() + Duration::from_millis(30),
            &std::sync::atomic::AtomicBool::new(false),
        );
        assert!(matches!(
            result,
            Err(super::DecoderWaitError::DeadlineExceeded)
        ));
        assert!(child.try_wait()?.is_some());
        Ok(())
    }

    #[test]
    fn decoder_exit_wait_preserves_cancellation_and_reaps_the_child() -> Result<(), Box<dyn Error>>
    {
        let (_directory, mut child) = sleeping_decoder()?;
        let result = super::wait_for_decoder(
            &mut child,
            Instant::now(),
            &std::sync::atomic::AtomicBool::new(true),
        );
        assert!(matches!(result, Err(super::DecoderWaitError::Cancelled)));
        assert!(child.try_wait()?.is_some());
        Ok(())
    }
}
