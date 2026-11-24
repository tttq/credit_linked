// Test program to read ZCode authentication
use credit_core::adapters::zcode_local;

fn main() {
    println!("=== ZCode Authentication Test ===\n");
    
    match zcode_local::read_live_auth() {
        Some(auth) => {
            println!("✓ Successfully loaded authentication!");
            println!("  UID: {}", auth.uid);
            println!("  Name: {:?}", auth.name);
            println!("  Device Mid: {}", auth.device_mid);
            println!("  JWT present: {}", auth.zcode_jwt.is_some());
            println!("  Access token length: {}", auth.access_token.len());
            
            if let Some(jwt) = &auth.zcode_jwt {
                println!("  JWT length: {}", jwt.len());
            }
        }
        None => {
            println!("✗ Failed to load authentication");
            println!("  This may be due to:");
            println!("  1. Credential file not found");
            println!("  2. OS key retrieval failed");
            println!("  3. Decryption failed (wrong key?)");
        }
    }
}
