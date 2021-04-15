fn main() {
    cc::Build::new()
        .file("src/fflog.c")
        .compile("fflog");
    // I couldn't find an easy way to avoid building this outside of Linux. I
    // believe it will be harmless, since it has no dependencies (other than
    // the ones fflog already has) and will simply be discarded on non-Linux
    // targets.
    cc::Build::new()
        .file("src/alsalog.c")
        .compile("alsalog");
}
