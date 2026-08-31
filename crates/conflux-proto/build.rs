fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build scripts run single-threaded before any code that reads
    // this env var (prost-build, invoked below).
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    // Two schemas, one package. `fl_transport.proto` is the hop every
    // deployment uses; `trusted_reference.proto` is the optional
    // server<->sidecar hop ADR 0011 added, kept in its own file so a
    // reader can see at a glance that it is a separate contract and not
    // part of the client-facing surface.
    tonic_prost_build::configure().compile_protos(
        &["proto/fl_transport.proto", "proto/trusted_reference.proto"],
        &["proto/"],
    )?;

    Ok(())
}
