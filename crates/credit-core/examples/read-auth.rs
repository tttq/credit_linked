//! ZCode 凭据读取工具
//! 
//! 用法：cargo run --release --example read-auth

use credit_core::adapters::zcode_local;

fn main() {
    println!("=========================================");
    println!("ZCode 4.0.0 凭据读取工具");
    println!("=========================================");
    println!();

    // 尝试读取本机认证信息
    match zcode_local::read_live_auth() {
        Some(auth) => {
            println!("✅ 成功读取认证信息");
            println!();
            println!("用户信息:");
            println!("  UID: {}", auth.uid);
            if let Some(name) = &auth.name {
                println!("  Name: {}", name);
            }
            println!();
            println!("设备信息:");
            println!("  Device Mid: {}", auth.device_mid);
            println!();
            println!("令牌信息:");
            
            if let Some(jwt) = &auth.zcode_jwt {
                println!("  zcodejwttoken: {}... (length: {})", 
                    jwt.chars().take(20).collect::<String>(), 
                    jwt.len());
                
                // 输出到环境变量供后续脚本使用
                println!();
                println!("💡 设置环境变量:");
                println!("  $env:ZCODE_JWT='{}'", jwt);
            } else {
                println!("  ⚠️  zcodejwttoken 未找到");
            }
            
            println!("  access_token: {}... (length: {})", 
                auth.access_token.chars().take(20).collect::<String>(), 
                auth.access_token.len());
            
            println!();
            println!("=========================================");
            println!("提示：复制上面的 token 到 test-zcode-4.0.ps1 中使用");
            println!("=========================================");
        }
        None => {
            eprintln!("❌ 无法读取认证信息");
            eprintln!();
            eprintln!("可能的原因:");
            eprintln!("  1. ZCode 客户端未登录");
            eprintln!("  2. 凭据文件不存在：~/.zcode/v2/credentials.json");
            eprintln!("  3. Windows Credential Manager 中没有加密密钥");
            eprintln!();
            eprintln!("解决方法:");
            eprintln!("  - 确保 ZCode 客户端已登录");
            eprintln!("  - 或设置环境变量 ZCODE_CREDENTIAL_SECRET");
            std::process::exit(1);
        }
    }
}
