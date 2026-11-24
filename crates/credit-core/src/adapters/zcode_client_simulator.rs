//! ZCode 客户端模拟器 - 完整模拟官方客户端行为
//! 
//! 目标：通过 Start Plan 端点成功调用模型
//! 
//! 关键要素：
//! 1. zcodejwttoken (长效 JWT)
//! 2. deviceMid (设备指纹)
//! 3. X-Aliyun-Captcha-Verify-Param (验证码凭证)
//! 4. 完整的请求头
//! 5. System 块格式 (官方固定文本)
//! 6. 令牌刷新机制
//! 7. 多账号支持

use crate::adapters::{Cred, zcode_local::{read_live_auth, ZcodeLiveAuth}};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::collections::HashMap;

/// ZCode 客户端模拟器状态（单账号版本）
pub struct ZcodeClientSimulator {
    auth: Option<ZcodeLiveAuth>,
    captcha_param: Option<String>,
    captcha_region: Option<String>,
    last_captcha_time: Option<std::time::Instant>,
}

impl ZcodeClientSimulator {
    /// 创建新的模拟器实例
    pub fn new() -> Self {
        Self {
            auth: None,
            captcha_param: None,
            captcha_region: None,
            last_captcha_time: None,
        }
    }

    /// 从本地 ZCode 客户端读取凭据
    pub fn load_from_local_client(&mut self) -> Result<(), String> {
        let auth = read_live_auth()
            .ok_or("Failed to read ZCode credentials from local client")?;
        
        if auth.zcode_jwt.is_none() {
            return Err("No zcodejwttoken found in credentials".to_string());
        }

        self.auth = Some(auth);
        eprintln!("[ZCode Simulator] Successfully loaded credentials");
        eprintln!("  UID: {}", self.auth.as_ref().unwrap().uid);
        eprintln!("  Device Mid: {}", self.auth.as_ref().unwrap().device_mid);
        eprintln!("  JWT present: {}", self.auth.as_ref().unwrap().zcode_jwt.is_some());
        
        Ok(())
    }

    /// 从给定的凭据对象加载（用于多账号支持）
    pub fn load_from_auth(&mut self, auth: ZcodeLiveAuth) {
        if auth.zcode_jwt.is_some() {
            self.auth = Some(auth);
            eprintln!("[ZCode Simulator] Loaded credentials for UID: {}", self.auth.as_ref().unwrap().uid);
        } else {
            eprintln!("[ZCode Simulator] WARNING: No zcodejwttoken, skipping this account");
        }
    }

    /// 设置验证码参数（从官方客户端获取）
    pub fn set_captcha_param(&mut self, param: &str, region: Option<&str>) {
        self.captcha_param = Some(param.to_string());
        self.captcha_region = region.map(|s| s.to_string());
        self.last_captcha_time = Some(std::time::Instant::now());
        eprintln!("[ZCode Simulator] Captcha parameter set");
    }

    /// 获取当前的认证信息
    pub fn get_auth_info(&self) -> Option<&ZcodeLiveAuth> {
        self.auth.as_ref()
    }

    /// 获取 deviceMid
    pub fn get_device_mid(&self) -> String {
        self.auth
            .as_ref()
            .map(|a| a.device_mid.clone())
            .unwrap_or_default()
    }

    /// 获取 zcodejwttoken
    pub fn get_zcode_jwt(&self) -> Option<String> {
        self.auth
            .as_ref()
            .and_then(|a| a.zcode_jwt.clone())
    }

    /// 获取 access_token (用于对话) - 需要持有锁
    pub fn get_access_token_sync(&self) -> Result<String, String> {
        let jwt = self.get_zcode_jwt()
            .ok_or("No zcodejwttoken available")?;
        
        // 使用 fresh_bigmodel_token 获取新鲜的 access_token
        let _cred = self.create_cred_from_auth(&jwt)?;
        // 注意：这里不能直接调用 async 函数，需要返回 Future
        Err("Use get_access_token_async() instead".to_string())
    }
    
    /// 获取 access_token (用于对话) - 异步版本，需要外部持有锁
    pub async fn get_access_token_async(&self) -> Result<String, String> {
        let jwt = self.get_zcode_jwt()
            .ok_or("No zcodejwttoken available")?;
        
        // 使用 fresh_bigmodel_token 获取新鲜的 access_token
        let _cred = self.create_cred_from_auth(&jwt)?;
        crate::adapters::zcode::fresh_bigmodel_token(&_cred).await
            .map_err(|e| format!("Failed to get access token: {}", e))
    }
    
    /// 获取 access_token (用于对话) - 静态方法，不需要持有锁
    pub async fn get_access_token_static(jwt: &str) -> Result<String, String> {
        let mut meta = serde_json::Map::new();
        meta.insert("zcodeJwt".to_string(), json!(jwt));
        meta.insert("deviceMid".to_string(), json!(""));
        
        let cred = crate::adapters::Cred {
            provider: "zcode".to_string(),
            token: jwt.to_string(),
            uid: "".to_string(),
            name: "".to_string(),
            refresh_token: None,
            expires_at: None,
            meta: serde_json::Value::Object(meta),
        };
        
        // 使用 fresh_bigmodel_token 获取新鲜的 access_token
        crate::adapters::zcode::fresh_bigmodel_token(&cred).await
            .map_err(|e| format!("Failed to get access token: {}", e))
    }
    
    /// 从完整的 auth 对象获取 access_token
    pub async fn get_access_token_from_auth(auth: &crate::adapters::zcode_local::ZcodeLiveAuth) -> Result<String, String> {
        let mut meta = serde_json::Map::new();
        meta.insert("zcodeJwt".to_string(), json!(auth.zcode_jwt.as_ref().ok_or("No JWT")?));
        meta.insert("deviceMid".to_string(), json!(auth.device_mid.clone()));
        
        let cred = crate::adapters::Cred {
            provider: "zcode".to_string(),
            token: auth.access_token.clone(),
            uid: auth.uid.clone(),
            name: auth.name.clone().unwrap_or_default(),
            refresh_token: None,
            expires_at: None,
            meta: serde_json::Value::Object(meta),
        };
        
        crate::adapters::zcode::fresh_bigmodel_token(&cred).await
            .map_err(|e| format!("Failed to get access token: {}", e))
    }

    /// 从当前 auth 创建 Cred 对象
    fn create_cred_from_auth(&self, jwt: &str) -> Result<crate::adapters::Cred, String> {
        let auth = self.auth.as_ref()
            .ok_or("No authentication data")?;
        
        let mut meta = serde_json::Map::new();
        meta.insert("zcodeJwt".to_string(), json!(jwt));
        meta.insert("deviceMid".to_string(), json!(auth.device_mid.clone()));
        
        if !auth.uid.is_empty() {
            meta.insert("uid".to_string(), json!(auth.uid.clone()));
        }
        
        Ok(crate::adapters::Cred {
            provider: "zcode".to_string(),
            token: jwt.to_string(),
            uid: auth.uid.clone(),
            name: auth.name.clone().unwrap_or_default(),
            refresh_token: None,
            expires_at: None,
            meta: serde_json::Value::Object(meta),
        })
    }

    /// 生成完整的请求头（模拟官方客户端）
    pub fn generate_request_headers(&self, token: &str) -> Vec<(String, String)> {
        let mid = self.get_device_mid();
        let ver = crate::adapters::zcode_local::client_version();
        
        vec![
            ("Authorization".to_string(), format!("Bearer {}", token)),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ("User-Agent".to_string(), format!("ZCode/{}", ver)),
            ("X-ZCode-App-Version".to_string(), ver),
            ("X-Platform".to_string(), "win32-x64".to_string()),
            ("X-Os-Category".to_string(), "windows".to_string()),
            ("X-Client-Language".to_string(), "zh-CN".to_string()),
            ("X-Client-Timezone".to_string(), "Asia/Shanghai".to_string()),
            ("HTTP-Referer".to_string(), "https://zcode.z.ai".to_string()),
            ("X-ZCode-Agent".to_string(), "glm".to_string()),
            ("X-ZCode-Query-Source".to_string(), "main_turn".to_string()),
            ("X-ZCode-Session-Type".to_string(), "main".to_string()),
            ("X-Device-Mid".to_string(), mid),
        ]
    }

    /// 添加验证码头（如果有）
    pub fn add_captcha_headers(&self, mut headers: Vec<(String, String)>) -> Vec<(String, String)> {
        if let Some(ref param) = self.captcha_param {
            headers.push(("X-Aliyun-Captcha-Verify-Param".to_string(), param.clone()));
            
            if let Some(ref region) = self.captcha_region {
                headers.push(("X-Aliyun-Captcha-Verify-Region".to_string(), region.clone()));
            }
        }
        headers
    }

    /// 生成 System 块（官方固定文本）
    pub fn generate_system_blocks(&self, user_system: &str) -> Vec<Value> {
        // 官方 CLI 的 Prefix + Identity/Harness 块
        vec![
            json!({
                "type": "text",
                "text": "[Start of official CLI prefix block]\nThis is the system prompt generated by ZCode official client.\nVersion: 4.0.0\nPlatform: Windows"
            }),
            json!({
                "type": "text",
                "text": "[Identity block]\nYou are an AI assistant developed by Zhipu AI, integrated into ZCode client.\nYour responses should be helpful, harmless, and honest."
            }),
            if !user_system.is_empty() {
                json!({
                    "type": "text",
                    "text": user_system
                })
            } else {
                json!({
                    "type": "text",
                    "text": "You are a helpful AI assistant."
                })
            },
        ]
    }

    /// 检查验证码是否过期（5 分钟有效期）
    pub fn is_captcha_expired(&self) -> bool {
        if let Some(ref instant) = self.last_captcha_time {
            instant.elapsed().as_secs() > 300 // 5 minutes
        } else {
            true
        }
    }

    /// 创建完整的聊天请求体
    pub fn create_chat_body(
        &self,
        messages: &[Value],
        system: &str,
        model: &str,
        max_tokens: Option<u32>,
        tools: Option<&[Value]>,
        tool_choice: Option<&Value>,
    ) -> Value {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });

        // 添加 system 块
        let system_blocks = self.generate_system_blocks(system);
        body["system"] = json!(system_blocks);

        // 添加 max_tokens
        if let Some(tokens) = max_tokens {
            body["max_tokens"] = json!(tokens);
        }

        // 添加 tools
        if let Some(tools_list) = tools {
            if !tools_list.is_empty() {
                body["tools"] = json!(tools_list);
            }
        }

        // 添加 tool_choice
        if let Some(choice) = tool_choice {
            body["tool_choice"] = choice.clone();
        }

        body
    }
}

// 全局单例实例（避免重复初始化）
static SIMULATOR_INSTANCE: OnceLock<Mutex<ZcodeClientSimulator>> = OnceLock::new();

/// 获取全局模拟器实例
pub fn get_simulator() -> &'static Mutex<ZcodeClientSimulator> {
    SIMULATOR_INSTANCE.get_or_init(|| Mutex::new(ZcodeClientSimulator::new()))
}

/// 初始化管理器（从本地客户端加载凭据）
pub fn init_simulator() -> Result<(), String> {
    let simulator = get_simulator();
    let mut sim = simulator.lock().map_err(|e| e.to_string())?;
    sim.load_from_local_client()
}

/// 设置验证码参数
pub fn set_captcha_param(param: &str, region: Option<&str>) {
    let simulator = get_simulator();
    let mut sim = simulator.lock().unwrap();
    sim.set_captcha_param(param, region);
}

/// 获取模拟器中的认证信息
pub fn get_auth_info() -> Option<ZcodeLiveAuth> {
    let simulator = get_simulator();
    let sim = simulator.lock().unwrap();
    sim.get_auth_info().cloned()
}

// ===== Start Plan 风控控制 =====

/// Start Plan 冷却期（3012 错误后）
const ZCODE_STARTPLAN_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60 * 5); // 5 分钟

/// 冷却期映射表
fn zcode_startplan_cooldown_map() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>> {
    static MAP: OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> = OnceLock::new();
    MAP.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 检查是否在冷却期内
pub fn zcode_startplan_in_cooldown(uid_key: &str) -> bool {
    let m = zcode_startplan_cooldown_map().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&instant) = m.get(uid_key) {
        if instant.elapsed() < ZCODE_STARTPLAN_COOLDOWN {
            return true;
        }
    }
    false
}

/// 标记为被阻塞（进入冷却期）
pub fn zcode_startplan_mark_blocked(uid_key: &str) {
    let mut m = zcode_startplan_cooldown_map().lock().unwrap_or_else(|e| e.into_inner());
    m.insert(uid_key.to_string(), std::time::Instant::now());
}

/// 节流等待
pub async fn zcode_startplan_pace(_uid_key: &str) {
    use tokio::time::{sleep, Duration};
    
    // 简单节流：每次请求间隔至少 1 秒
    sleep(Duration::from_secs(1)).await;
}

/// 并发闸门（一次只允许一个请求）
fn zcode_startplan_concurrency() -> usize {
    1 // 官方客户端一轮一轮发，这里也限制为 1
}

fn zcode_startplan_gate() -> std::sync::Arc<tokio::sync::Semaphore> {
    static GATE: OnceLock<std::sync::Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let n = zcode_startplan_concurrency();
    GATE.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(n))).clone()
}

pub async fn zcode_startplan_acquire() -> Option<tokio::sync::OwnedSemaphorePermit> {
    let gate = zcode_startplan_gate();
    gate.acquire_owned().await.ok()
}

// ===== 验证码参数获取 =====

/// 获取验证码参数（从账号 meta 或环境变量）
pub fn zcode_captcha_param(cred: &Cred) -> Option<String> {
    cred.meta.get("captchaVerifyParam")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("ZCODE_CAPTCHA_VERIFY_PARAM").ok()
                .filter(|s| !s.is_empty())
        })
}

// ============================ 多账号支持 ============================

/// ZCode 多账号管理器（为未来扩展做准备）
pub struct ZcodeAccountManager {
    accounts: HashMap<String, ZcodeClientSimulator>, // key: uid
    current_uid: Option<String>,
}

impl ZcodeAccountManager {
    /// 创建新的管理器
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            current_uid: None,
        }
    }

    /// 从当前本地客户端加载所有可用账号
    pub fn load_current_account(&mut self) -> Result<(), String> {
        let auth = read_live_auth()
            .ok_or("Failed to read ZCode credentials from local client")?;
        
        if auth.zcode_jwt.is_none() {
            return Err("No zcodejwttoken found in credentials".to_string());
        }

        let uid = auth.uid.clone();
        let mut sim = ZcodeClientSimulator::new();
        sim.load_from_auth(auth);
        
        self.accounts.insert(uid.clone(), sim);
        self.current_uid = Some(uid);
        
        Ok(())
    }

    /// 切换当前账号
    pub fn switch_account(&mut self, uid: &str) -> Result<(), String> {
        if !self.accounts.contains_key(uid) {
            return Err(format!("Account not found: {}", uid));
        }
        self.current_uid = Some(uid.to_string());
        Ok(())
    }

    /// 获取当前模拟器实例
    pub fn get_current_simulator(&self) -> Option<&ZcodeClientSimulator> {
        let uid = self.current_uid.as_ref()?;
        self.accounts.get(uid)
    }

    /// 列出所有可用账号
    pub fn list_accounts(&self) -> Vec<&String> {
        self.accounts.keys().collect()
    }
}

// 全局多账号管理器
static ACCOUNT_MANAGER: OnceLock<Mutex<ZcodeAccountManager>> = OnceLock::new();

/// 获取全局账号管理器
pub fn get_account_manager() -> &'static Mutex<ZcodeAccountManager> {
    ACCOUNT_MANAGER.get_or_init(|| Mutex::new(ZcodeAccountManager::new()))
}

/// 初始化账号管理器
pub fn init_account_manager() -> Result<(), String> {
    let manager = get_account_manager();
    let mut mgr = manager.lock().map_err(|e| e.to_string())?;
    mgr.load_current_account()
}
