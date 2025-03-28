//! Simple implementation of a gRPC server for the Hierophant prover network
use hyper::service::{make_service_fn, service_fn};
use std::convert::Infallible;
use tonic::transport::Server;

// Import modules
mod http_handler;
mod services;

// Include the generated proto code
pub mod network {
    tonic::include_proto!("network");
}

pub mod artifact {
    tonic::include_proto!("artifact");
}

use artifact::artifact_store_server::ArtifactStoreServer;
use http_handler::handle_http_request;
use network::prover_network_server::ProverNetworkServer;
use services::{ArtifactStoreService, ProverNetworkService};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define the server addresses
    let grpc_addr = ([0, 0, 0, 0], 9009).into();
    let http_addr = ([0, 0, 0, 0], 9010).into();

    // Create our services
    let prover_service = ProverNetworkService::new();
    let artifact_service = ArtifactStoreService::new();

    // Get a reference to the upload URLs for the HTTP handler
    let upload_urls = artifact_service.get_upload_urls();

    println!("Starting Hierophant services:");
    println!("  - gRPC server on http://0.0.0.0:9009");
    println!("  - HTTP server on http://0.0.0.0:9010");
    println!("Implemented methods:");
    println!("  - network.ProverNetwork/GetProgram");
    println!("  - artifact.ArtifactStore/CreateArtifact");
    println!("  - HTTP POST/PUT to /upload/* (for artifact uploads)");

    // Run the HTTP server in a separate task
    let http_server = tokio::spawn(async move {
        let make_svc = make_service_fn(move |_conn| {
            let upload_urls = upload_urls.clone();
            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    handle_http_request(req, upload_urls.clone())
                }))
            }
        });

        hyper::Server::bind(&http_addr)
            .serve(make_svc)
            .await
            .unwrap();
    });

    // Run the gRPC server
    let grpc_server = Server::builder()
        .add_service(ProverNetworkServer::new(prover_service))
        .add_service(ArtifactStoreServer::new(artifact_service))
        .serve(grpc_addr);

    println!("Servers started. Press Ctrl+C to stop.");

    // Wait for both servers to complete (or error)
    tokio::select! {
        _ = grpc_server => println!("gRPC server terminated"),
        _ = http_server => println!("HTTP server terminated"),
    }

    Ok(())
}
