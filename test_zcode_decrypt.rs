// Quick test to verify ZCode credential decryption
use credit_core::adapters::zcode_local::read_live_auth;

fn main() {
    println!("=== Testing ZCode Credential Decryption ===\n");
    
    match read_live_auth() {
        Some(auth) => {
            println!("✓ Successfully loaded credentials!");
            println!("\nAuth Info:");
            println!("  UID: {}", auth.uid);
            println!("  Name: {:?}", auth.name);
            println!("  Device Mid: {}", auth.device_mid);
            println!("  Access Token length: {}", auth.access_token.len());
            println!("  JWT present: {}", auth.zcode_jwt.is_some());
            
            if let Some(ref jwt) = auth.zcode_jwt {
                println!("  JWT length: {} chars", jwt.len());
            } else {
                println!("\n⚠️  WARNING: zcodejwttoken is missing or empty!");
                println!("   This will cause Start Plan requests to fail.");
            }
        }
        None => {
            println!("✗ Failed to load credentials");
            println!("\nPossible reasons:");
            println!("  1. ZCode client not logged in");
            println!("  2. Credentials file not found");
            println!("  3. Windows Credential Manager key not available");
            println!("\nTo fix:");
            println!("  - Make sure ZCode client is installed and logged in");
            println!("  - Check that credentials exist in Windows Credential Manager");
            println!("  - Or set ZCODE_CREDENTIAL_SECRET environment variable as workaround");
        }
    }
}
