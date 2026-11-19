//! Write cumulative local reports without doing file I/O in the editor event loop.

use std::{
    io::Write,
    path::Path,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

pub(super) struct Reporter {
    stop: mpsc::Sender<()>,
    worker: thread::JoinHandle<()>,
}

impl Reporter {
    pub(super) fn start() -> Option<Self> {
        let directory = std::env::var_os("MITOS_PERFORMANCE_DIR")?;
        match Self::spawn(Path::new(&directory)) {
            Ok(reporter) => Some(reporter),
            Err(error) => {
                log::warn!("could not start performance recording: {error}");
                None
            }
        }
    }

    fn spawn(directory: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        let file = tempfile::Builder::new()
            .prefix("mitos-performance-")
            .suffix(".json")
            .tempfile_in(directory)?;
        let (stop, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("mitos-performance".into())
            .spawn(move || {
                let recorder = view::performance::enable();
                let Ok((_, path)) = file.keep().inspect_err(|error| {
                    log::warn!("could not retain performance report: {error}");
                }) else {
                    return;
                };
                log::info!("writing performance report to {}", path.display());
                loop {
                    let write_report = || -> anyhow::Result<()> {
                        let directory = path.parent().expect("report has a parent directory");
                        let mut output = tempfile::NamedTempFile::new_in(directory)?;
                        serde_json::to_writer_pretty(&mut output, &recorder.report())?;
                        output.write_all(b"\n")?;
                        output.persist(&path)?;
                        Ok(())
                    };
                    if let Err(error) = write_report() {
                        log::warn!("could not write performance report: {error}");
                    }
                    if receiver.recv_timeout(Duration::from_secs(60))
                        != Err(RecvTimeoutError::Timeout)
                    {
                        if let Err(error) = write_report() {
                            log::warn!("could not write final performance report: {error}");
                        }
                        break;
                    }
                }
            })?;
        // Install the recorder before the first input can be handled.
        view::performance::enable();
        Ok(Self { stop, worker })
    }

    pub(super) fn finish(self) {
        let _ = self.stop.send(());
        if self.worker.join().is_err() {
            log::warn!("performance report writer stopped unexpectedly");
        }
    }
}
