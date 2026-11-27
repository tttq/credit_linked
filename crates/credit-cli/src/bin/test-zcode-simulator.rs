// 测试 ZCode 客户端模拟器
use credit_core::adapters::zcode_client_simulator::{init_simulator, get_auth_info};
use serde_json::json;

#[tokio::main]
async fn main() {
    println!("=== ZCode Client Simulator Test ===\n");
    
    // 1. 初始化模拟器（从本地客户端读取凭据）
    println!("Step 1: Loading credentials from local ZCode client...");
    match init_simulator() {
        Ok(_) => println!("✓ Credentials loaded successfully\n"),
        Err(e) => {
            eprintln!("✗ Failed to load credentials: {}", e);
            eprintln!("\nPlease ensure:");
            eprintln!("  1. ZCode client is installed at D:\\ProgramFiles\\develop\\ZCode\\ZCode.exe");
            eprintln!("  2. You are logged in to ZCode client");
            eprintln!("  3. Credentials exist at %USERPROFILE%\\.zcode\\v2\\credentials.json");
            return;
        }
    }
    
    // 2. 获取认证信息
    println!("Step 2: Authentication Info:");
    if let Some(auth) = get_auth_info() {
        println!("  UID: {}", auth.uid);
        println!("  Name: {:?}", auth.name);
        println!("  Device Mid: {}", auth.device_mid);
        println!("  JWT present: {}", auth.zcode_jwt.is_some());
        println!("  Access Token length: {}", auth.access_token.len());
        
        if let Some(jwt) = &auth.zcode_jwt {
            println!("  JWT length: {} chars", jwt.len());
        }
    } else {
        eprintln!("✗ No authentication info available");
        return;
    }
    
    // 3. 设置验证码参数（需要从官方客户端获取）
    println!("\nStep 3: Captcha Parameter (optional but recommended)");
    println!("  Note: This parameter must be obtained from the official ZCode client");
    println!("  Run a message in the official client first to generate it.");
    println!("  Example: set_captcha_param(\"your_param_here\", \"cn\")");
    
    // 4. 创建聊天请求示例
    println!("\nStep 4: Creating chat request body...");
    
    let messages = [
        json!({
            "role": "user",
            "content": "你好，请介绍一下你自己"
        })
    ];
    
    let system = "你是一个有用的 AI 助手";
    let model = "GLM-5.3-Flash";
    let max_tokens = 2000u32;
    
    println!("  Messages: {:?}", messages[0]);
    println!("  System: {}", system);
    println!("  Model: {}", model);
    println!("  Max Tokens: {}", max_tokens);
    
    println!("\n=== Test Complete ===");
    println!("\nNext steps:");
    println!("  1. Use the simulator in your gateway code");
    println!("  2. Call get_access_token() to get fresh token");
    println!("  3. Use generate_request_headers() for complete headers");
    println!("  4. Set captcha param if required by server");
}
