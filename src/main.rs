extern crate cargo_deb;
use cargo_deb::*;

extern crate getopts;

use std::env;
use std::path::Path;
use std::process;
use std::time;

struct CliOptions {
    no_build: bool,
    no_strip: bool,
    verbose: bool,
    quiet: bool,
    install: bool,
    output_path: Option<String>,
    variant: Option<String>,
    target: Option<String>,
    manifest_path: Option<String>,
    cargo_build_flags: Vec<String>,
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut cli_opts = getopts::Options::new();
    cli_opts.optflag("", "no-build", "Assume project is already built");
    cli_opts.optflag("", "no-strip", "Do not strip debug symbols from the binary");
    cli_opts.optflag("", "install", "Immediately install created package");
    cli_opts.optopt("", "target", "Rust target for cross-compilation", "triple");
    cli_opts.optopt("", "variant", "Alternative configuration section to use", "name");
    cli_opts.optopt("", "manifest-path", "Cargo project file location", "./Cargo.toml");
    cli_opts.optopt("o", "output", "Write .deb to this file or directory", "path");
    cli_opts.optflag("q", "quiet", "Don't print warnings");
    cli_opts.optflag("v", "verbose", "Print progress");
    cli_opts.optflag("h", "help", "Print this help menu");
    cli_opts.optflag("", "version", "Show the version of cargo-deb");

    let matches = match cli_opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(err) => {
            err_exit(&err);
        },
    };
    if matches.opt_present("h") {
        print!("{}", cli_opts.usage("Usage: cargo deb [options] [-- <cargo build flags>]"));
        return;
    }

    if matches.opt_present("version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }

    match process(CliOptions {
        no_build: matches.opt_present("no-build"),
        no_strip: matches.opt_present("no-strip"),
        quiet: matches.opt_present("quiet"),
        verbose: matches.opt_present("verbose"),
        install: matches.opt_present("install"),
        variant: matches.opt_str("variant"),
        target: matches.opt_str("target"),
        output_path: matches.opt_str("output"),
        manifest_path: matches.opt_str("manifest-path"),
        cargo_build_flags: matches.free,
    }) {
        Ok(()) => {},
        Err(err) => {
            err_exit(&err);
        }
    }
}

fn err_cause(err: &std::error::Error, max: usize) {
    if let Some(reason) = err.cause() {
        eprintln!("  because: {}", reason);
        if max > 0 {
            err_cause(reason, max - 1);
        }
    }
}

fn err_exit(err: &std::error::Error) -> ! {
    eprintln!("cargo-deb: {}", err);
    err_cause(err, 3);
    process::exit(1);
}

fn process(CliOptions {manifest_path, output_path, variant, target, install, no_build, no_strip, quiet, verbose, mut cargo_build_flags}: CliOptions) -> CDResult<()> {
    let target = target.as_ref().map(|s|s.as_str());
    let variant = variant.as_ref().map(|s| s.as_str());

    // `cargo deb` invocation passes the `deb` arg through.
    if cargo_build_flags.first().map_or(false, |arg| arg == "deb") {
        cargo_build_flags.remove(0);
    }

    // Listener conditionally prints warnings
    let mut listener_tmp1;
    let mut listener_tmp2;
    let listener: &mut listener::Listener = if quiet {
        listener_tmp1 = listener::NoOpListener;
        &mut listener_tmp1
    } else {
        listener_tmp2 = listener::StdErrListener {verbose};
        &mut listener_tmp2
    };

    let manifest_path = manifest_path.as_ref().map(|s|s.as_str()).unwrap_or("Cargo.toml");
    let mut options = Config::from_manifest(Path::new(manifest_path), output_path, target, variant, listener)?;
    reset_deb_directory(&options)?;

    if !no_build {
        cargo_build(&options, target, &cargo_build_flags, verbose)?;
    }

    options.resolve_assets()?;

    if options.strip && !no_strip {
        strip_binaries(&options, target, listener)?;
    }

    // Obtain the current time which will be used to stamp the generated files in the archives.
    let system_time = time::SystemTime::now().duration_since(time::UNIX_EPOCH)?.as_secs();
    let mut deb_contents = DebArchive::new(&options)?;

    deb_contents.add_data("debian-binary", system_time, b"2.0\n")?;

    // The block frees the large data_archive var early
    {
        // Initailize the contents of the data archive (files that go into the filesystem).
        let (data_archive, asset_hashes) = data::generate_archive(&options, system_time, listener)?;

        // Initialize the contents of the control archive (metadata for the package manager).
        let control_archive = control::generate_archive(&options, system_time, asset_hashes, listener)?;

        // Order is important for Debian
        deb_contents.add_data("control.tar.gz", system_time, &compress::gz(&control_archive)?)?;
        match compress::xz_or_gz(&data_archive)? {
            compress::Compressed::Gz(data) => deb_contents.add_data("data.tar.gz", system_time, &data)?,
            compress::Compressed::Xz(data) => deb_contents.add_data("data.tar.xz", system_time, &data)?,
        }
    }

    let generated = deb_contents.finish()?;
    if !quiet {
        println!("{}", generated.display());
    }

    if install {
        install_deb(&generated)?;
    }
    Ok(())
}
