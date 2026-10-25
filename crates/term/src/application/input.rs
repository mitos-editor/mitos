//! Terminal input with an acknowledged pause before an external program takes over.

use std::{
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    task::{Context, Poll},
    thread,
};

use futures_util::Stream;
use tokio::sync::mpsc;

#[derive(Default)]
struct State {
    paused: AtomicBool,
    stopped: AtomicBool,
    reading: Mutex<()>,
    wake: Condvar,
    interrupt: Mutex<Option<termina::event::PlatformWaker>>,
}

#[derive(Clone, Default)]
pub(super) struct Control(Arc<State>);

impl Control {
    /// The worker holds this lock while reading. Acquiring it acknowledges that
    /// no read remains in flight, rather than merely suppressing delivered events.
    pub fn pause(&self) -> Paused {
        self.0.paused.store(true, Ordering::SeqCst);
        self.interrupt();
        drop(self.0.reading.lock().unwrap());
        Paused(self.clone())
    }

    fn interrupt(&self) {
        if let Some(wake) = self.0.interrupt.lock().unwrap().as_ref() {
            let _ = wake.wake();
        }
    }

    fn resume(&self) {
        let _guard = self.0.reading.lock().unwrap();
        self.0.paused.store(false, Ordering::SeqCst);
        self.0.wake.notify_one();
    }
}

#[must_use = "keep the guard alive while the terminal is handed off"]
pub(super) struct Paused(Control);

impl Drop for Paused {
    fn drop(&mut self) {
        self.0.resume();
    }
}

type ReadEvent = Box<dyn FnMut() -> io::Result<Option<termina::Event>> + Send>;

pub(super) struct Events {
    receiver: mpsc::Receiver<io::Result<termina::Event>>,
    control: Control,
    worker: Option<thread::JoinHandle<()>>,
    reader: Option<ReadEvent>,
    sender: Option<mpsc::Sender<io::Result<termina::Event>>>,
}

impl Events {
    pub fn new(reader: termina::EventReader, control: Control) -> Self {
        *control.0.interrupt.lock().unwrap() = Some(reader.waker());
        Self::with_reader(control, move || {
            let filter = |event: &termina::Event| {
                !event.is_escape()
                    || matches!(
                        event,
                        termina::Event::Csi(termina::escape::csi::Csi::Mode(
                            termina::escape::csi::Mode::ReportTheme(_)
                        ))
                    )
            };
            if reader.poll(None, filter)? {
                reader.read(filter).map(Some)
            } else {
                Ok(None)
            }
        })
    }

    fn with_reader(
        control: Control,
        read: impl FnMut() -> io::Result<Option<termina::Event>> + Send + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(128);
        Self {
            receiver,
            control,
            worker: None,
            reader: Some(Box::new(read)),
            sender: Some(sender),
        }
    }

    // Startup capability probes read the terminal synchronously. Do not compete
    // with them: start reading only when the application first polls for input.
    fn start(&mut self) {
        let Some(mut read) = self.reader.take() else {
            return;
        };
        let sender = self.sender.take().unwrap();
        let state = self.control.clone();
        self.worker = Some(thread::spawn(move || loop {
            let mut reading = state.0.reading.lock().unwrap();
            while state.0.paused.load(Ordering::SeqCst) && !state.0.stopped.load(Ordering::SeqCst) {
                reading = state.0.wake.wait(reading).unwrap();
            }
            if state.0.stopped.load(Ordering::SeqCst) {
                break;
            }
            let event = read();
            let failed = event.is_err();
            // Backpressure must not hold the read lock: pause acknowledges only
            // terminal reads, and does not depend on the frontend draining input.
            drop(reading);
            if let Some(event) = event.transpose()
                && sender.blocking_send(event).is_err()
            {
                break;
            }
            if failed {
                break;
            }
        }));
    }
}

impl Stream for Events {
    type Item = io::Result<termina::Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.start();
        self.receiver.poll_recv(cx)
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        self.receiver.close();
        self.control.0.stopped.store(true, Ordering::SeqCst);
        self.control.interrupt();
        let guard = self.control.0.reading.lock().unwrap();
        self.control.0.wake.notify_one();
        drop(guard);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::atomic::AtomicUsize, time::Duration};

    #[test]
    fn pause_waits_for_the_reader_and_drop_joins_a_paused_worker() {
        let control = Control::default();
        let reads = Arc::new(AtomicUsize::new(0));
        let (entered, started) = std::sync::mpsc::channel();
        let worker_reads = reads.clone();
        let mut events = Events::with_reader(control.clone(), move || {
            worker_reads.fetch_add(1, Ordering::SeqCst);
            let _ = entered.send(());
            thread::sleep(Duration::from_millis(1));
            Ok(None)
        });
        events.start();
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        let pause = control.pause();
        let paused = reads.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(reads.load(Ordering::SeqCst), paused);
        drop(pause);
        while started.try_recv().is_ok() {}
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        let _pause = control.pause();
        drop(events);
    }

    #[test]
    fn a_full_input_queue_does_not_prevent_pause_or_shutdown() {
        let control = Control::default();
        let (full, ready) = std::sync::mpsc::channel();
        let mut count = 0;
        let mut events = Events::with_reader(control.clone(), move || {
            count += 1;
            if count == 129 {
                full.send(()).unwrap();
            }
            Ok(Some(termina::Event::FocusIn))
        });
        events.start();
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let (ack, paused) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let guard = control.pause();
            let _ = ack.send(());
            guard
        });
        let acknowledged = paused.recv_timeout(Duration::from_secs(1));
        // Close the queue before joining, even if the assertion fails.
        drop(events);
        drop(worker.join().unwrap());
        acknowledged.unwrap();
    }
}
