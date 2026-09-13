use nih_plug::wrapper::standalone::nih_export_standalone_with_args;

use akifx::AkiFx;

fn main() {
    // Raise the default period size above nih-plug's 512-sample default: some
    // WASAPI devices deliver packets larger than the size we request (e.g.
    // 1056 samples with a ~22 ms device period), and nih-plug's cpal backend
    // asserts when that happens. 2048 covers realistic shared-mode device
    // periods. An explicit `--period-size`/`-p` on the command line still
    // takes precedence.
    let mut args: Vec<String> = std::env::args().collect();
    let user_set_period_size = args
        .iter()
        .any(|arg| arg == "--period-size" || arg == "-p");
    if !user_set_period_size {
        args.push("--period-size".to_owned());
        args.push("2048".to_owned());
    }

    let success = nih_export_standalone_with_args::<AkiFx, _>(args);
    std::process::exit(if success { 0 } else { 1 });
}
