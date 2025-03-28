use hyper::{Body, Method, Request, Response, StatusCode};
use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

// HTTP handler for file uploads
pub async fn handle_http_request(
    req: Request<Body>,
    upload_urls: Arc<Mutex<HashSet<String>>>,
) -> Result<Response<Body>, Infallible> {
    let path = req.uri().path().to_string();
    let method = req.method().clone();

    // Check if this is an upload request to a valid path
    if path.starts_with("/upload/") && (method == Method::PUT || method == Method::POST) {
        // Check if this is a valid upload URL
        let is_valid = upload_urls.lock().unwrap().contains(&path);

        if !is_valid {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("Invalid upload URL"))
                .unwrap());
        }

        println!("\n=== Received Upload Request ===");
        println!("Path: {}", path);
        println!("Method: {}", method);

        // Print headers
        println!("Headers:");
        for (name, value) in req.headers() {
            println!("  {}: {}", name, value.to_str().unwrap_or("[binary]"));
        }

        // Read the request body
        match hyper::body::to_bytes(req.into_body()).await {
            Ok(bytes) => {
                println!("Received upload data: {} bytes", bytes.len());

                // Print a preview of the data
                if !bytes.is_empty() {
                    let preview_size = std::cmp::min(100, bytes.len());
                    println!(
                        "Data preview (first {} bytes): {}",
                        preview_size,
                        hex::encode(&bytes[..preview_size])
                    );
                }

                // Return success
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from("Upload successful"))
                    .unwrap())
            }
            Err(e) => {
                println!("Error reading upload data: {}", e);

                Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from(format!("Upload failed: {}", e)))
                    .unwrap())
            }
        }
    } else {
        // For any other request, return not found
        Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from(format!("Not found: {}", path)))
            .unwrap())
    }
}
