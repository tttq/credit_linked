// Test program to check ZCode version detection
use credit_core::adapters::zcode_local;

fn main() {
    println!("=== ZCode Version Detection Test ===\n");
    
    let version = zcode_local::client_version();
    println!("Detected ZCode version: {}", version);
    
    // Also test find_client_exe
    if let Some(exe) = zcode_local::find_client_exe() {
        println!("Client executable: {:?}", exe);
    } else {
        println!("Client executable not found");
    }
}
