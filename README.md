![Stable Rust CI Status](https://github.com/rot256/hjul/workflows/stable/badge.svg)
![Beta Rust CI Status](https://github.com/rot256/hjul/workflows/beta/badge.svg)
![Nightly Rust CI Status](https://github.com/rot256/hjul/workflows/nightly/badge.svg)

# Hjul

Hjul is a thin wrapper around `mio-extra` timers. Example usage:

```rust
use hjul::Runner;
use std::thread;
use std::time::Duration;

let runner = Runner::new(Duration::from_millis(100), 100, 1024);
let timer = runner.timer(|| println!("fired"));

timer.start(Duration::from_millis(200));

// wait for timer to fire
thread::sleep(Duration::from_millis(500));

// start the timer again
timer.start(Duration::from_millis(200));

// stop timer immediately
timer.stop();

// start the timer again
timer.start(Duration::from_millis(200));

// timer is stopped when it goes out of scope
```

Full documentation at [docs.rs/hjul](https://docs.rs/hjul/)
