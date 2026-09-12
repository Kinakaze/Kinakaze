//! Own only processes started by this command, and reap them on every exit path.

use crate::{Result, failure};
use kinakaze_v2_host_win::ProcessHandle;
use std::io::{self, BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const HELPER_TIMEOUT: Duration = Duration::from_secs(20);

pub struct OwnedProcess {
    child: Child,
    output: Receiver<io::Result<Option<String>>>,
    exited: Receiver<io::Result<()>>,
    label: &'static str,
}

impl OwnedProcess {
    pub fn spawn(command: &mut Command, label: &'static str) -> Result<Self> {
        command
            .creation_flags(kinakaze_v2_host_win::background_creation_flags())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn()?;
        let process = match ProcessHandle::open(child.id()) {
            Ok(process) => process,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        };
        let stdout = child.stdout.take().expect("piped child stdout");
        let (output_tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if output_tx.send(line.map(Some)).is_err() {
                    return;
                }
            }
            let _ = output_tx.send(Ok(None));
        });
        let (exit_tx, exited) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = exit_tx.send(process.wait());
        });
        Ok(Self {
            child,
            output,
            exited,
            label,
        })
    }

    pub fn expect_ready(&self) -> Result<()> {
        match self.output.recv_timeout(HELPER_TIMEOUT) {
            Ok(Ok(Some(line))) if line.trim() == "READY" => Ok(()),
            Ok(Ok(Some(_))) => Err(failure(format!("{} did not send READY", self.label))),
            Ok(Ok(None)) => Err(failure(format!("{} exited before READY", self.label))),
            Ok(Err(error)) => Err(error.into()),
            Err(_) => Err(failure(format!("{} readiness timed out", self.label))),
        }
    }

    pub fn finish(&mut self) -> Result<()> {
        self.exited
            .recv_timeout(HELPER_TIMEOUT)
            .map_err(|_| failure(format!("{} exit timed out", self.label)))??;
        let status = self.child.wait()?;
        loop {
            match self.output.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(Some(line))) => println!("{line}"),
                Ok(Ok(None)) => break,
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => return Err(failure(format!("{} output did not close", self.label))),
            }
        }
        if status.success() {
            Ok(())
        } else {
            Err(failure(format!("{} failed with {status}", self.label)))
        }
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
