use anyhow::{Context as _, Result, anyhow, bail};
use std::io::Read;
use std::process::Child;
use std::thread::{self, JoinHandle};

pub(super) struct ManagedChild {
    child: Option<Child>,
    stderr_thread: Option<JoinHandle<std::io::Result<String>>>,
}

impl ManagedChild {
    pub(super) fn new(mut child: Child) -> Result<Self> {
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("child stderr was not piped"))?;
        let stderr_thread = thread::spawn(move || {
            let mut text = String::new();
            stderr.read_to_string(&mut text)?;
            Ok(text)
        });
        Ok(Self {
            child: Some(child),
            stderr_thread: Some(stderr_thread),
        })
    }

    fn collect_stderr(&mut self) -> Result<String> {
        let Some(thread) = self.stderr_thread.take() else {
            return Ok(String::new());
        };
        thread
            .join()
            .map_err(|_| anyhow!("ffmpeg stderr drain thread panicked"))?
            .context("reading ffmpeg stderr")
    }

    pub(super) fn wait_success(&mut self, name: &str) -> Result<()> {
        let status = match self.child.as_mut() {
            Some(child) => child
                .wait()
                .with_context(|| format!("waiting for {name}"))?,
            None => bail!("{name} has already been reaped"),
        };
        self.child.take();
        let stderr = self.collect_stderr()?;
        if !status.success() {
            bail!("{name} failed: {}", stderr.trim());
        }
        Ok(())
    }

    pub(super) fn cancel(&mut self) {
        if let Some(mut child) = self.child.take()
            && matches!(child.try_wait(), Ok(None) | Err(_))
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.collect_stderr();
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn drains_large_stderr_before_waiting_for_child() {
        let child = Command::new("sh")
            .args([
                "-c",
                "i=0; while [ $i -lt 20000 ]; do printf x >&2; i=$((i + 1)); done",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let mut managed = ManagedChild::new(child).unwrap();
        assert!(managed.wait_success("test helper").is_ok());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn drop_kills_and_reaps_a_running_child() {
        let child = Command::new("sh")
            .args(["-c", "while :; do printf x >&2; sleep 1; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let managed = ManagedChild::new(child).unwrap();
        drop(managed);
        assert!(
            !Command::new("kill")
                .args(["-0", &pid.to_string()])
                .output()
                .unwrap()
                .status
                .success(),
            "child {pid} is still running"
        );
    }
}
