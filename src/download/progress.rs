use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

/// Create a multi-progress bar manager for downloads.
pub fn create_multi_progress() -> MultiProgress {
    MultiProgress::new()
}

/// Create the main progress bar for overall download progress.
pub fn create_main_bar(total: u64) -> ProgressBar {
    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} ({percent}%) {msg}",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏  "),
    );
    bar.set_message("downloading...");
    bar
}

/// Create a progress bar for a single file download (bytes).
pub fn create_download_bar(filename: &str) -> ProgressBar {
    let bar = ProgressBar::new_spinner();
    bar.set_style(
        ProgressStyle::with_template("  {spinner:.blue} {msg}")
            .unwrap(),
    );
    bar.set_message(filename.to_string());
    bar
}
