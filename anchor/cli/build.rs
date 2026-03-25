fn main() {
    // This build script exists solely to ensure that `OUT_DIR` is set at compile time,
    // which is used by `build_profile_name()` to determine the build profile.
    // See https://stackoverflow.com/questions/73595435/how-to-get-profile-from-cargo-toml-in-build-rs-or-at-runtime
}
