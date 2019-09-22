use mio::{Events, Poll, PollOpt, Ready, Token};
use mio_extras::timer;

use spin::Mutex;

use std::fmt;
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::Duration;

const TIMER: Token = Token(1);

type State = Weak<TimerInner>;

struct TimerInner {
    timer: Arc<Mutex<timer::Timer<State>>>,
    pending: AtomicBool,
    timeout: Mutex<Option<timer::Timeout>>,
    callback: Box<dyn Fn() + Send + Sync>,
}

impl fmt::Debug for TimerInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Timer {{ pending = {} }} ",
            self.pending.load(Ordering::Acquire)
        )
    }
}

pub struct Runner {
    timer: Arc<Mutex<timer::Timer<State>>>,
    handle: Option<thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl fmt::Debug for Runner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Runner {{ running = {} }} ",
            self.running.load(Ordering::Acquire)
        )
    }
}

#[derive(Clone)]
pub struct Timer(Arc<TimerInner>);

impl fmt::Debug for Timer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Runner {
    /// Creates a new Runner, which executes the associated timer callbacks
    ///
    /// # Arguments
    ///
    /// * `tick`: duration of a single tick. This determines the accuracy of the underlaying timer wheel
    /// * `slots`: Number of slots in the timer wheel.
    /// * `capacity`: Maximum number of timers which can be allocated for the wheel
    ///
    /// # Note
    ///
    /// The longest possible duration of any timer is `tick` * `slots`
    ///
    /// # Example
    ///
    /// ```
    /// use hjul::Runner;
    /// use std::time::Duration;
    ///
    /// // allows 1024 timers, with duration up to 10s and 100ms accuracy
    /// let runner = Runner::new(Duration::from_millis(100), 100, 1024);
    /// ```
    pub fn new(tick: Duration, slots: usize, capacity: usize) -> Runner {
        // create timer whell
        let builder: timer::Builder = Default::default();
        let builder = timer::Builder::tick_duration(builder, tick);
        let builder = timer::Builder::num_slots(builder, slots);
        let builder = timer::Builder::capacity(builder, capacity);
        let timer = timer::Builder::build(builder);

        // create Poll
        let poll = Poll::new().unwrap();
        poll.register(&timer, TIMER, Ready::readable(), PollOpt::level())
            .unwrap();

        // allow sharing state
        let timer = Arc::new(Mutex::new(timer));
        let running = Arc::new(AtomicBool::new(true));

        // start callback thread
        let handle = {
            let timer = timer.clone();
            let running = running.clone();
            thread::spawn(move || {
                let mut events = Events::with_capacity(256);
                while running.load(Ordering::Acquire) {
                    poll.poll(&mut events, None).unwrap();
                    for event in &events {
                        match event.token() {
                            TIMER => {
                                if let Some(weak) = timer.lock().poll() {
                                    let weak: Weak<TimerInner> = weak;
                                    match weak.upgrade() {
                                        Some(timer) => {
                                            if timer.pending.swap(false, Ordering::SeqCst) {
                                                (timer.callback)()
                                            }
                                        }
                                        None => (),
                                    }
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            })
        };

        // return runner handle
        Runner {
            timer,
            handle: Some(handle),
            running,
        }
    }

    /// Allocate a new (stopped) timer and associate it with the callback
    ///
    /// # Arguments
    ///
    /// * `callback`: Callback to execute whenever the timer fires (possible repeatedly, if reset).
    ///
    /// # Example
    ///
    /// ```
    /// # use hjul::Runner;
    /// # use std::thread;
    /// # use std::time::Duration;
    /// # let runner = Runner::new(Duration::from_millis(100), 100, 1024);
    /// let timer = runner.timer(|| println!("fired"));
    ///
    /// // start the timer
    /// timer.reset(Duration::from_millis(100));
    ///
    /// // wait for timer to fire
    /// thread::sleep(Duration::from_millis(1000));
    /// ```
    pub fn timer<F>(&self, callback: F) -> Timer
    where
        F: 'static + Fn() + Send + Sync,
    {
        Timer(Arc::new(TimerInner {
            callback: Box::new(callback),
            pending: AtomicBool::new(false),
            timer: self.timer.clone(),
            timeout: Mutex::new(None),
        }))
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        // mark the runner as stopped
        self.running.store(false, Ordering::SeqCst);

        // create an event for mio, causing the callback thread to be scheduled
        self.timer
            .lock()
            .set_timeout(Duration::from_millis(0), Weak::new());

        // join with the callback thread
        if let Some(handle) = mem::replace(&mut self.handle, None) {
            handle.join().unwrap();
        }
    }
}

impl TimerInner {
    fn stop(&self) {
        if self.pending.swap(false, Ordering::Acquire) {
            if let Some(tm) = mem::replace(&mut *self.timeout.lock(), None) {
                self.timer.lock().cancel_timeout(&tm);
            }
        }
    }

    fn fire(&self) {
        self.stop();
        (self.callback)()
    }
}

impl Timer {
    /// Stop the timer (preventing execution of the callback in the future)
    ///
    /// # Note
    ///
    /// Another way to stop a timer is to drop every clone of the timer.
    ///
    /// # Example
    ///
    /// ```
    /// # use hjul::Runner;
    /// # use std::thread;
    /// # use std::time::Duration;
    /// # let runner = Runner::new(Duration::from_millis(100), 100, 1024);
    /// let timer = runner.timer(|| assert!(false));
    /// timer.reset(Duration::from_millis(200));
    /// timer.stop();
    ///
    /// // callback is never executed
    /// thread::sleep(Duration::from_millis(500));
    /// ```
    pub fn stop(&self) {
        self.0.stop()
    }

    /// Restart the timer, regardless of whether the timer is running or not.
    /// e.g. repeatably calling .reset(1 sec) will cause the timer to never fire.
    ///
    /// # Arguments
    ///
    /// * `duration`: duration until the callback should execute
    ///
    /// # Example
    ///
    /// ```
    /// # use hjul::Runner;
    /// # use std::thread;
    /// # use std::time::Duration;
    /// # let runner = Runner::new(Duration::from_millis(100), 100, 1024);
    /// let timer = runner.timer(|| assert!(false));
    ///
    /// // the timer never fires
    /// let dur = Duration::from_millis(200);
    /// for _ in 0..5 {
    ///     timer.reset(dur);
    ///     thread::sleep(dur / 2);
    /// }
    ///
    /// // timer is dropped and cancelled
    /// ```
    pub fn reset(&self, duration: Duration) {
        let inner = &self.0;

        inner.pending.store(true, Ordering::SeqCst);
        let mut timeout = inner.timeout.lock();
        let mut timer = inner.timer.lock();
        let new = timer.set_timeout(duration, Arc::downgrade(&self.0));
        if let Some(tm) = mem::replace(&mut *timeout, Some(new)) {
            timer.cancel_timeout(&tm);
        }
    }

    /// Start the timer, but only if the timer is not already pending
    /// e.g. if repeatably calling .start(1 sec), the timer will fire ~ once every second
    ///
    /// # Arguments
    ///
    /// * `duration`: duration until the callback should execute
    ///
    /// # Returns
    ///
    /// A bool indicating whether the timer was started (true)
    /// or already running (false).
    ///
    /// # Example
    ///
    /// ```
    /// # use hjul::Runner;
    /// # use std::thread;
    /// # use std::time::Duration;
    /// # let runner = Runner::new(Duration::from_millis(100), 100, 1024);
    /// let timer = runner.timer(|| println!("fired"));
    ///
    /// // this timer will fire twice
    /// let dur = Duration::from_millis(200);
    /// for _ in 0..5 {
    ///     timer.start(dur);
    ///     thread::sleep(dur / 2);
    /// }
    ///
    /// // timer is dropped and cancelled
    /// ```
    pub fn start(&self, duration: Duration) -> bool {
        // optimistic check for pending
        let inner = &self.0;
        if inner.pending.load(Ordering::Acquire) {
            return false;
        }

        // take lock and set if not pending
        let mut timeout = inner.timeout.lock();
        let mut timer = inner.timer.lock();
        if inner.pending.load(Ordering::Acquire) {
            return false;
        }
        *timeout = Some(timer.set_timeout(duration, Arc::downgrade(&self.0)));
        inner.pending.store(true, Ordering::SeqCst);
        true
    }

    /// Manually cause the timer to fire immediately.
    /// This cancels any pending timeout (equivalent to calling .stop())
    /// before executing the callback.
    ///
    /// # Note
    ///
    /// The callback is run in the calling thread
    /// as oppose to being run by the thread in the associated `Runner` instance.
    ///
    /// # Example
    ///
    /// ```
    /// # use hjul::Runner;
    /// # use std::thread;
    /// # use std::time::Duration;
    /// # let runner = Runner::new(Duration::from_millis(100), 100, 1024);
    /// let timer = runner.timer(|| println!("fired"));
    ///
    /// timer.start(Duration::from_millis(200));
    /// timer.fire();
    ///
    /// // timer is fired immediately, not after 200ms.
    /// ```
    pub fn fire(&self) {
        self.0.fire();
    }
}

impl Drop for TimerInner {
    fn drop(&mut self) {
        self.stop()
    }
}
