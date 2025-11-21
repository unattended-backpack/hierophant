fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/network.proto")?;
    tonic_build::compile_protos("proto/artifact.proto")?;
    Ok(())
}
