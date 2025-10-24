use std::{future::Future, time::Duration};

use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

pub struct Progress {
    multi: MultiProgress,
    style: ProgressStyle,
}

impl Progress {
    pub fn stdout() -> Self {
        let multi = MultiProgress::with_draw_target(ProgressDrawTarget::stdout());
        let style = ProgressStyle::with_template("{prefix} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner());
        Self { multi, style }
    }

    pub fn step(&self, label: impl Into<String>) -> Step {
        let bar = self.multi.add(ProgressBar::new_spinner());
        bar.set_style(self.style.clone());
        Step::new(bar, label.into())
    }
}

pub struct Step {
    bar: ProgressBar,
    label: String,
}

impl Step {
    fn new(bar: ProgressBar, label: String) -> Self {
        bar.set_prefix("");
        bar.set_message("");
        Self { bar, label }
    }

    pub fn clone_bar(&self) -> ProgressBar {
        self.bar.clone()
    }

    fn start(&self, steady_tick: Option<Duration>) {
        self.bar.set_prefix("[…]");
        self.bar.set_message(self.label.clone());
        match steady_tick {
            Some(interval) => self.bar.enable_steady_tick(interval),
            None => self.bar.tick(),
        }
    }

    fn finish_success(&self, message: String) {
        self.bar.disable_steady_tick();
        self.bar.set_prefix("[✔]");
        self.bar.finish_with_message(message);
    }

    fn finish_failure(&self) {
        self.bar.disable_steady_tick();
        self.bar.set_prefix("[✖]");
        self.bar
            .finish_with_message(format!("{} (failed)", self.label));
    }

    pub async fn run<F, T, S>(
        &self,
        steady_tick: Option<Duration>,
        future: F,
        success_message: S,
    ) -> Result<T>
    where
        F: Future<Output = Result<T>>,
        S: FnOnce(&T) -> String,
    {
        self.start(steady_tick);
        match future.await {
            Ok(value) => {
                let message = success_message(&value);
                self.finish_success(message);
                Ok(value)
            }
            Err(err) => {
                self.finish_failure();
                Err(err)
            }
        }
    }
}
