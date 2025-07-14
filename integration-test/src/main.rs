fn main() {
    println!("Hello, world!");
}

// #[tokio::test]
// async fn test_proof_generation() {
//     // Start Hierophant
//     let mut hierophant = Command::new("cargo")
//         .args(&["run", "--release", "--bin", "hierophant"])
//         .spawn()
//         .expect("Failed to start Hierophant");
//
//     // Wait for Hierophant to be ready
//     sleep(Duration::from_secs(2)).await;
//
//     // Start Contemplant
//     let mut contemplant = Command::new("cargo")
//         .args(&["run", "--release", "--bin", "contemplant"])
//         .spawn()
//         .expect("Failed to start Contemplant");
//
//     // Your test logic here...
//
//     // Cleanup
//     hierophant.kill().ok();
//     contemplant.kill().ok();
// }
