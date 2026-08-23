fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build scripts run single-threaded before any code that reads
    // this env var (prost-build, invoked below).
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    tonic_prost_build::configure().compile_protos(&["proto/fl_transport.proto"], &["proto/"])?;

    Ok(())
}
