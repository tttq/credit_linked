import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
const WASM_FILE = process.env.QODER_WASM ?? (dirname(fileURLToPath(import.meta.url)) + '/qoder_auth_wasm_bg.wasm')
const w = { log: () => {}, warn: () => {}, debug: () => {}, error: () => {} }
globalThis.__QODER_AUTH_EXPORTS = {}
var dEi = 'qoder-auth-wasm'
const _$d1=(s,k="B8CeTxpHukdh")=>{const b=Buffer.from(s,"base64");for(let i=0;i<b.length;i++)b[i]^=k.charCodeAt(i%k.length);return b.toString()}
function ayc(A){let e,t;try{let g=Mt.__wbindgen_add_to_stack_pointer(-16),l=eE(A,Mt.__wbindgen_export2,Mt.__wbindgen_export3),B=Pg;Mt.generate_runtime_auth_fields(g,l,B);var i=oo().getInt32(g+0,!0),n=oo().getInt32(g+4,!0),r=oo().getInt32(g+8,!0),o=oo().getInt32(g+12,!0),s=i,a=n;if(o)throw s=0,a=0,hx(r);return e=s,t=a,mD(s,a)}finally{Mt.__wbindgen_add_to_stack_pointer(16),Mt.__wbindgen_export4(e,t,1)}}
function JA(name, exports) { globalThis.__QODER_AUTH_GETTERS = exports }
const __MODULE_INITS = []
function C(init) { __MODULE_INITS.push(init); let done = false, out; return () => { if (!done) { done = true; init(); } return out } }
function __RUN_ALL_MODULES() { for (const init of __MODULE_INITS) { try { init() } catch {} } }
function lEi(){return{__proto__:null,"./qoder_auth_wasm_bg.js":{__proto__:null,__wbg_Error_2e59b1b37a9a34c3:function(A,e){return nw(Error(mD(A,e)))},__wbg___wbindgen_is_function_49868bde5eb1e745:function(A){return"function"==typeof kI(A)},__wbg___wbindgen_is_object_40c5a80572e8f9d3:function(A){let e=kI(A);return"object"==typeof e&&null!==e},__wbg___wbindgen_is_string_b29b5c5a8065ba1a:function(A){return"string"==typeof kI(A)},__wbg___wbindgen_is_undefined_c0cca72b82b86f4d:function(A){return void 0===kI(A)},__wbg___wbindgen_throw_81fc77679af83bc6:function(A,e){throw new Error(mD(A,e))},__wbg_call_d578befcc3145dee:function(){return wuA(function(A,e,t){return nw(kI(A).call(kI(e),kI(t)))},arguments)},__wbg_crypto_38df2bab126b63dc:function(A){return nw(kI(A).crypto)},__wbg_getRandomValues_c44a50d8cfdaebeb:function(){return wuA(function(A,e){kI(A).getRandomValues(kI(e))},arguments)},__wbg_getRandomValues_d49329ff89a07af1:function(){return wuA(function(A,e){globalThis.crypto.getRandomValues(QGe(A,e))},arguments)},__wbg_length_0c32cb8543c8e4c8:function(A){return kI(A).length},__wbg_msCrypto_bd5a034af96bcba6:function(A){return nw(kI(A).msCrypto)},__wbg_new_99cabae501c0a8a0:function(){return nw(new Map)},__wbg_new_with_length_9cedd08484b73942:function(A){return nw(new Uint8Array(A>>>0))},__wbg_node_84ea875411254db1:function(A){return nw(kI(A).node)},__wbg_now_88621c9c9a4f3ffc:function(){return Date.now()},__wbg_process_44c7a14e11e9f69e:function(A){return nw(kI(A).process)},__wbg_prototypesetcall_3e05eb9545565046:function(A,e,t){Uint8Array.prototype.set.call(QGe(A,e),kI(t))},__wbg_randomFillSync_6c25eac9869eb53c:function(){return wuA(function(A,e){kI(A).randomFillSync(hx(e))},arguments)},__wbg_require_b4edbdcf3e2a1ef0:function(){return wuA(function(){return nw(module.require)},arguments)},__wbg_set_08463b1df38a7e29:function(A,e,t){return nw(kI(A).set(kI(e),kI(t)))},__wbg_static_accessor_GLOBAL_THIS_a1248013d790bf5f:function(){let A=typeof globalThis>"u"?null:globalThis;return V$(A)?0:nw(A)},__wbg_static_accessor_GLOBAL_f2e0f995a21329ff:function(){let A=typeof global>"u"?null:global;return V$(A)?0:nw(A)},__wbg_static_accessor_SELF_24f78b6d23f286ea:function(){let A=typeof self>"u"?null:self;return V$(A)?0:nw(A)},__wbg_static_accessor_WINDOW_59fd959c540fe405:function(){let A=typeof window>"u"?null:window;return V$(A)?0:nw(A)},__wbg_subarray_0f98d3fb634508ad:function(A,e,t){return nw(kI(A).subarray(e>>>0,t>>>0))},__wbg_versions_276b2795b1c6a219:function(A){return nw(kI(A).versions)},__wbindgen_cast_0000000000000001:function(A,e){return nw(QGe(A,e))},__wbindgen_cast_0000000000000002:function(A,e){return nw(mD(A,e))},__wbindgen_object_clone_ref:function(A){return nw(kI(A))},__wbindgen_object_drop_ref:function(A){hx(A)}}}}
function nw(A){DuA===j$.length&&j$.push(j$.length+1);let e=DuA;return DuA=j$[e],j$[e]=A,e}function Eyc(A){A<1028||(j$[A]=DuA,DuA=A)}function QGe(A,e){return A>>>=0,CGe().subarray(A/1,A/1+e)}function oo(){return(null===ESe||!0===ESe.buffer.detached||void 0===ESe.buffer.detached&&ESe.buffer!==Mt.memory.buffer)&&(ESe=new DataView(Mt.memory.buffer)),ESe}function mD(A,e){return Iyc(A>>>=0,e)}function CGe(){return(null===SuA||0===SuA.byteLength)&&(SuA=new Uint8Array(Mt.memory.buffer)),SuA}function kI(A){return j$[A]}function wuA(A,e){try{return A.apply(this,e)}catch(A){Mt.__wbindgen_export(nw(A))}}function V$(A){return null==A}function _un(A,e){let t=e(1*A.length,1)>>>0;return CGe().set(A,t/1),Pg=A.length,t}function eE(A,e,t){if(void 0===t){let t=yuA.encode(A),i=e(t.length,1)>>>0;return CGe().subarray(i,i+t.length).set(t),Pg=t.length,i}let i=A.length,n=e(i,1)>>>0,r=CGe(),o=0;for(;o<i;o++){let e=A.charCodeAt(o);if(e>127)break;r[n+o]=e}if(o!==i){0!==o&&(A=A.slice(o)),n=t(n,i,i=o+3*A.length,1)>>>0;let e=CGe().subarray(n+o,n+i);o+=yuA.encodeInto(A,e).written,n=t(n,i,o,1)>>>0}return Pg=o,n}function hx(A){let e=kI(A);return Eyc(A),e}function Iyc(A,e){return(zun+=e)>=hyc&&((c3A=new TextDecoder("utf-8",{ignoreBOM:!0,fatal:!0})).decode(),zun=e),c3A.decode(CGe().subarray(A,A+e))}function uEi(A,e){return Mt=A.exports,Byc=e,ESe=null,SuA=null,Mt}async function Qyc(A,e){if("function"==typeof Response&&A instanceof Response){if("function"==typeof WebAssembly.instantiateStreaming)try{return await WebAssembly.instantiateStreaming(A,e)}catch(e){if(!A.ok||!function(A){switch(A){case"basic":case"cors":case"default":return!0}return!1}(A.type)||"application/wasm"===A.headers.get("Content-Type"))throw e;console.warn(_$d1("Im8mBxULAy0YCQgRbFEtFiAZHjwcChANEUwxADUVGSYSC0QOI1EvADBYEi0WChEbJxg6CiEKUDsQGRINMBgnCjELUCYaH0QbJ0o1AHQvETsYSxMBNlBjBTUIACQcCAUcK1ctSiMZAyUVSykhD31jES0IFWZVLQUELlEtAnQaESseSxAHYlgUADY5AzsQBgYEOxYqCycMESYBAgUcJ1hjEjwREyBVAhdIMVQsEjEKXmg6GQ0PK1YiCXQdAjoaGV5i"),e)}let t=await A.arrayBuffer();return await WebAssembly.instantiate(t,e)}{let t=await WebAssembly.instantiate(A,e);return t instanceof WebAssembly.Instance?{instance:t,module:A}:t}}function Cyc(A){if(void 0!==Mt)return Mt;void 0!==A&&(Object.getPrototypeOf(A)===Object.prototype?({module:A}=A):console.warn(_$d1("N0sqCzNYFC0FGQELI0wmAXQIEToUBgEcJ0owRTIXAmgVAgoBNms6CzdQWShOSxQJMUtjBHQLGSYSBwFILVopADcMUCEbGBANI1w=")));let e=lEi();return A instanceof WebAssembly.Module||(A=new WebAssembly.Module(A)),uEi(new WebAssembly.Instance(A,e),A)}async function wyc(A){if(void 0!==Mt)return Mt;void 0!==A&&(Object.getPrototypeOf(A)===Object.prototype?({module_or_path:A}=A):console.warn(_$d1("N0sqCzNYFC0FGQELI0wmAXQIEToUBgEcJ0owRTIXAmgBAwFIK1YqET0ZHCEPChABLVZjAyEWEzwcBApTYkgiFidYEWgGAgoPLl1jCjYSFSsBSw0GMUwmBDA="))),void 0===A&&(A=new URL("qoder_auth_wasm_bg.wasm",import.meta.url));let e=lEi();("string"==typeof A||"function"==typeof Request&&A instanceof Request||"function"==typeof URL&&A instanceof URL)&&(A=fetch(A));let{instance:t,module:i}=await Qyc(await A,e);return uEi(t,i)}JA(dEi,{ClientContext:()=>PuA,ProfileEncryptor:()=>kuA,QoderContext:()=>xuA,RequestResult:()=>hSe,build_httpdns_url:()=>ryc,credential_storage_decrypt:()=>iyc,credential_storage_encrypt:()=>oyc,decrypt_server_response:()=>syc,default:()=>wyc,generate_runtime_auth_fields:()=>ayc,get_external_providers_feature_gate_fallback:()=>cyc,get_httpdns_account_id:()=>lyc,get_httpdns_config:()=>uyc,get_httpdns_secret_key:()=>dyc,get_profile_key_fingerprint:()=>gyc,initSync:()=>Cyc,model_cache_decrypt:()=>fyc,model_cache_encrypt:()=>pyc,profile_encrypt:()=>myc});var PuA,kuA,xuA,hSe,oEi,sEi,aEi,cEi,ESe,SuA,j$,DuA,c3A,hyc,zun,yuA,Pg,Byc,Mt,
Pg,Byc,Mt,gEi=C(()=>{PuA=class{__destroy_into_raw(){let A=this.__wbg_ptr;return this.__wbg_ptr=0,oEi.unregister(this),A}free(){let A=this.__destroy_into_raw();Mt.__wbg_clientcontext_free(A,0)}constructor(A){let e=_un(A,Mt.__wbindgen_export2),t=Pg,i=Mt.clientcontext_new(e,t);return this.__wbg_ptr=i>>>0,oEi.register(this,this.__wbg_ptr,this),this}state(){return Mt.clientcontext_state(this.__wbg_ptr)>>>0}},Symbol.dispose&&(PuA.prototype[Symbol.dispose]=PuA.prototype.free),kuA=class{__destroy_into_raw(){let A=this.__wbg_ptr;return this.__wbg_ptr=0,sEi.unregister(this),A}free(){let A=this.__destroy_into_raw();Mt.__wbg_profileencryptor_free(A,0)}encryptChunk(A,e){try{let o=Mt.__wbindgen_add_to_stack_pointer(-16),s=_un(A,Mt.__wbindgen_export2),a=Pg;Mt.profileencryptor_encryptChunk(o,this.__wbg_ptr,s,a,e);var t=oo().getInt32(o+0,!0),i=oo().getInt32(o+4,!0),n=oo().getInt32(o+8,!0);if(oo().getInt32(o+12,!0))throw hx(n);var r=QGe(t,i).slice();return Mt.__wbindgen_export4(t,1*i,1),r}finally{Mt.__wbindgen_add_to_stack_pointer(16)}}header(){try{let i=Mt.__wbindgen_add_to_stack_pointer(-16);Mt.profileencryptor_header(i,this.__wbg_ptr);var A=oo().getInt32(i+0,!0),e=oo().getInt32(i+4,!0),t=QGe(A,e).slice();return Mt.__wbindgen_export4(A,1*e,1),t}finally{Mt.__wbindgen_add_to_stack_pointer(16)}}constructor(A){try{let i=Mt.__wbindgen_add_to_stack_pointer(-16);Mt.profileencryptor_new(i,A);var e=oo().getInt32(i+0,!0),t=oo().getInt32(i+4,!0);if(oo().getInt32(i+8,!0))throw hx(t);return this.__wbg_ptr=e>>>0,sEi.register(this,this.__wbg_ptr,this),this}finally{Mt.__wbindgen_add_to_stack_pointer(16)}}},Symbol.dispose&&(kuA.prototype[Symbol.dispose]=kuA.prototype.free),xuA=class{__destroy_into_raw(){let A=this.__wbg_ptr;return this.__wbg_ptr=0,aEi.unregister(this),A}free(){let A=this.__destroy_into_raw();Mt.__wbg_qodercontext_free(A,0)}get_external_providers_access(){let A,e;try{let n=Mt.__wbindgen_add_to_stack_pointer(-16);Mt.qodercontext_get_external_providers_access(n,this.__wbg_ptr);var t=oo().getInt32(n+0,!0),i=oo().getInt32(n+4,!0);return A=t,e=i,mD(t,i)}finally{Mt.__wbindgen_add_to_stack_pointer(16),Mt.__wbindgen_export4(A,e,1)}}constructor(A,e,t,i){try{let a=Mt.__wbindgen_add_to_stack_pointer(-16),g=eE(A,Mt.__wbindgen_export2,Mt.__wbindgen_export3),l=Pg,B=eE(e,Mt.__wbindgen_export2,Mt.__wbindgen_export3),c=Pg,Q=eE(t,Mt.__wbindgen_export2,Mt.__wbindgen_export3),I=Pg;var n=V$(i)?0:eE(i,Mt.__wbindgen_export2,Mt.__wbindgen_export3),r=Pg;Mt.qodercontext_new(a,g,l,B,c,Q,I,n,r);var o=oo().getInt32(a+0,!0),s=oo().getInt32(a+4,!0);if(oo().getInt32(a+8,!0))throw hx(s);return this.__wbg_ptr=o>>>0,aEi.register(this,this.__wbg_ptr,this),this}finally{Mt.__wbindgen_add_to_stack_pointer(16)}}prepareInferRequest(A,e,t,i){try{let l=Mt.__wbindgen_add_to_stack_pointer(-16),B=eE(A,Mt.__wbindgen_export2,Mt.__wbindgen_export3),c=Pg,Q=eE(e,Mt.__wbindgen_export2,Mt.__wbindgen_export3),I=Pg;var n=V$(t)?0:eE(t,Mt.__wbindgen_export2,Mt.__wbindgen_export3),r=Pg,o=V$(i)?0:eE(i,Mt.__wbindgen_export2,Mt.__wbindgen_export3),s=Pg;Mt.qodercontext_prepareInferRequest(l,this.__wbg_ptr,B,c,Q,I,n,r,o,s);var a=oo().getInt32(l+0,!0),g=oo().getInt32(l+4,!0);if(oo().getInt32(l+8,!0))throw hx(g);return hSe.__wrap(a)}finally{Mt.__wbindgen_add_to_stack_pointer(16)}}prepareRequest(A,e,t,i,n,r){try{let c=Mt.__wbindgen_add_to_stack_pointer(-16),Q=eE(A,Mt.__wbindgen_export2,Mt.__wbindgen_export3),I=Pg,E=eE(e,Mt.__wbindgen_export2,Mt.__wbindgen_export3),u=Pg,d=eE(t,Mt.__wbindgen_export2,Mt.__wbindgen_export3),C=Pg,w=eE(i,Mt.__wbindgen_export2,Mt.__wbindgen_export3),h=Pg;var o=V$(n)?0:eE(n,Mt.__wbindgen_export2,Mt.__wbindgen_export3),s=Pg,a=V$(r)?0:eE(r,Mt.__wbindgen_export2,Mt.__wbindgen_export3),g=Pg;Mt.qodercontext_prepareRequest(c,this.__wbg_ptr,Q,I,E,u,d,C,w,h,o,s,a,g);var l=oo().getInt32(c+0,!0),B=oo().getInt32(c+4,!0);if(oo().getInt32(c+8,!0))throw hx(B);return hSe.__wrap(l)}finally{Mt.__wbindgen_add_to_stack_pointer(16)}}refreshAuthFields(A){try{let t=Mt.__wbindgen_add_to_stack_pointer(-16),i=eE(A,Mt.__wbindgen_export2,Mt.__wbindgen_export3),n=Pg;Mt.qodercontext_refreshAuthFields(t,this.__wbg_ptr,i,n);var e=oo().getInt32(t+0,!0);if(oo().getInt32(t+4,!0))throw hx(e)}finally{Mt.__wbindgen_add_to_stack_pointer(16)}}},Symbol.dispose&&(xuA.prototype[Symbol.dispose]=xuA.prototype.free),hSe=class A{static __wrap(e){e>>>=0;let t=Object.create(A.prototype);return t.__wbg_ptr=e,cEi.register(t,t.__wbg_ptr,t),t}__destroy_into_raw(){let A=this.__wbg_ptr;return this.__wbg_ptr=0,cEi.unregister(this),A}free(){let A=this.__destroy_into_raw();Mt.__wbg_requestresult_free(A,0)}get body(){try{let t=Mt.__wbindgen_add_to_stack_pointer(-16);Mt.requestresult_body(t,this.__wbg_ptr);var A=oo().getInt32(t+0,!0),e=oo().getInt32(t+4,!0);let i;return 0!==A&&(i=mD(A,e).slice(),Mt.__wbindgen_export4(A,1*e,1)),i}finally{Mt.__wbindgen_add_to_stack_pointer(16)}}get headerCount(){return Mt.requestresult_headerCount(this.__wbg_ptr)>>>0}get headers(){return hx(Mt.requestresult_headers(this.__wbg_ptr))}get url(){let A,e;try{let n=Mt.__wbindgen_add_to_stack_pointer(-16);Mt.requestresult_url(n,this.__wbg_ptr);var t=oo().getInt32(n+0,!0),i=oo().getInt32(n+4,!0);return A=t,e=i,mD(t,i)}finally{Mt.__wbindgen_add_to_stack_pointer(16),Mt.__wbindgen_export4(A,e,1)}}},Symbol.dispose&&(hSe.prototype[Symbol.dispose]=hSe.prototype.free),oEi=typeof FinalizationRegistry>"u"?{register:()=>{},unregister:()=>{}}:new FinalizationRegistry(A=>Mt.__wbg_clientcontext_free(A>>>0,1)),sEi=typeof FinalizationRegistry>"u"?{register:()=>{},unregister:()=>{}}:new FinalizationRegistry(A=>Mt.__wbg_profileencryptor_free(A>>>0,1)),aEi=typeof FinalizationRegistry>"u"?{register:()=>{},unregister:()=>{}}:new FinalizationRegistry(A=>Mt.__wbg_qodercontext_free(A>>>0,1)),cEi=typeof FinalizationRegistry>"u"?{register:()=>{},unregister:()=>{}}:new FinalizationRegistry(A=>Mt.__wbg_requestresult_free(A>>>0,1)),ESe=null,SuA=null,(j$=new Array(1024).fill(void 0)).push(void 0,null,!0,!1),DuA=j$.length,(c3A=new TextDecoder("utf-8",{ignoreBOM:!0,fatal:!0})).decode(),hyc=2146435072,zun=0,"encodeInto"in(yuA=new TextEncoder)||(yuA.encodeInto=function(A,e){let t=yuA.encode(A);return e.set(t),{read:A.length,written:t.length}}),Pg=0})
import*as l3A from"node:fs/promises";import*as fEi from"node:path";function pEi(){return"function"==typeof process.geteuid?process.geteuid():void 0}function Syc(){return"function"==typeof process.getegid?process.getegid():void 0}function mEi(){return 0===pEi()}
// ===== 驱动 =====


import { readFileSync, writeFileSync as fsWrite } from 'node:fs'
import crypto from 'node:crypto'
async function loadContext(machineId, cosyVersion, userInfoJson) {
  const wasmBytes = readFileSync(WASM_FILE)
  const auth = await wyc(wasmBytes)
  const hMc = JSON.stringify({ uid: userInfoJson?.uid ?? '', encrypt_user_info: userInfoJson?.encrypt_user_info ?? '', key: userInfoJson?.key ?? '' })
  const settings = JSON.stringify({ client_type: '5', business_product: 'cli', business_type: 'agent', scene: 'assistant' })
  __RUN_ALL_MODULES()
  const g = globalThis.__QODER_AUTH_GETTERS ?? {}
  const QoderContext = g.QoderContext?.() ?? auth.QoderContext
  if (typeof QoderContext !== 'function') throw new Error('QoderContext class unavailable: ' + Object.keys(g).join(','))
  return new QoderContext(machineId, cosyVersion, hMc, settings)
}
let _wasmLoaded
async function ensureWasm() {
  if (!_wasmLoaded) {
    const wasmBytes = readFileSync(WASM_FILE)
    await wyc(wasmBytes)
    _wasmLoaded = true
  }
}
function getWasmInstance() {
  if (!Mt) throw new Error('wasm not initialized')
  const g = globalThis.__QODER_AUTH_GETTERS ?? {}
  let fn
  try { fn = g.generate_runtime_auth_fields() } catch (e) { throw new Error('getter eval: ' + e.message) }
  if (typeof fn !== 'function') throw new Error('generate_runtime_auth_fields not function: ' + typeof fn)
  return { generate_runtime_auth_fields: fn }
}
let _ctx
async function getContext() {
  if (!_ctx) {
    const machineId = process.env.QODER_MACHINE_ID ?? '00000000-0000-0000-0000-000000000000'
    const cosyVersion = process.env.QODER_VERSION ?? '0.5.13'
    const userInfo = process.env.QODER_USER_INFO ? JSON.parse(process.env.QODER_USER_INFO) : undefined
    _ctx = await loadContext(machineId, cosyVersion, userInfo)
  }
  return _ctx
}
export async function prepareWithToken(request) {
  // 真实凭据路径：device token → generate_runtime_auth_fields → QoderContext → prepareInferRequest
  __RUN_ALL_MODULES()
  await ensureWasm()
  const auth = globalThis.__QODER_AUTH_GETTERS ?? {}
  const wasm = getWasmInstance()
  // 1. 用 device token 拉 userinfo（uid/orgId）
  const uiResp = await fetch((request.openapiBase ?? 'https://openapi.qoder.com.cn') + '/api/v1/userinfo', {
    headers: { Authorization: 'Bearer ' + request.deviceToken, 'Cosy-ClientType': '10' },
    signal: AbortSignal.timeout(20000),
  })
  const ui = await uiResp.json()
  if (!uiResp.ok) throw new Error('userinfo HTTP ' + uiResp.status + ': ' + JSON.stringify(ui).slice(0, 200))
  const uid = String(ui.id ?? ui.user_id ?? ui.uid ?? '')
  if (!uid) throw new Error('userinfo missing uid')
  // 2. 生成 runtime auth fields
  const fieldsJson = wasm.generate_runtime_auth_fields(JSON.stringify({
    uid,
    security_oauth_token: request.deviceToken,
    organization_id: ui.orgId ?? ui.organization_id ?? '',
    organization_tags: ui.organization_tags ?? '',
    data_policy_agreed: true,
  }))
  const fields = JSON.parse(fieldsJson)
  // 3. 构造上下文（hMc = cachedUserInfo 全量）
  const hMc = JSON.stringify({
    uid, name: ui.name ?? '', email: ui.email ?? '', avatar_url: ui.avatar_url ?? '',
    organization_id: ui.orgId ?? ui.organization_id ?? '',
    security_oauth_token: request.deviceToken, access_token: request.deviceToken,
    login_method: 'token', login_timestamp: Date.now(),
    encrypt_user_info: fields.encrypt_user_info ?? '', key: fields.key ?? '',
  })
  const machineId = process.env.QODER_MACHINE_ID ?? '00000000-0000-0000-0000-000000000000'
  const cosyVersion = process.env.QODER_VERSION ?? '0.5.13'
  const settings = JSON.stringify({ client_type: '5', business_product: 'cli', business_type: 'agent', scene: 'assistant' })
  const ctx = new (auth.QoderContext())(machineId, cosyVersion, hMc, settings)
  // 4. 签名推理请求
  const result = ctx.prepareInferRequest(
    request.endpoint ?? '',
    typeof request.bodyJson === 'string' ? request.bodyJson : JSON.stringify(request.body ?? {}),
    request.modelKey ?? '',
    request.modelSource ?? '',
  )
  if (result.body === undefined) throw new Error('wasm returned no body')
  const rawHeaders = result.headerCount > 0 ? result.headers : new Map()
  const headers = rawHeaders instanceof Map ? Object.fromEntries(rawHeaders) : {}
  return { url: result.url, body: Buffer.from(result.body).toString('utf-8'), headers, uid }
}
export async function prepareInferRequest(request) {
  const ctx = await getContext()
  const result = ctx.prepareInferRequest(
    request.endpoint ?? '',
    typeof request.bodyJson === 'string' ? request.bodyJson : JSON.stringify(request.body ?? {}),
    request.modelKey ?? '',
    request.modelSource ?? '',
  )
  if (result.body === undefined) throw new Error('wasm returned no body')
  const rawHeaders = result.headerCount > 0 ? result.headers : new Map()
  const headers = rawHeaders instanceof Map ? Object.fromEntries(rawHeaders) : (typeof rawHeaders === 'string' ? JSON.parse(rawHeaders) : {})
  return { url: result.url, body: Buffer.from(result.body).toString('utf-8'), headers }
}

// ===== stdio 签名服务模式 =====
// 协议：stdin 每行一个 JSON 请求 {deviceToken, endpoint, bodyJson, modelKey, modelSource, machineId?}
//       stdout 每行一个 JSON 响应 {ok, url, body, headers} | {ok:false, error}
import { createInterface } from 'node:readline'
if (process.argv[2] === 'serve') {
  const rl = createInterface({ input: process.stdin, terminal: false })
  rl.on('line', async (line) => {
    if (!line.trim()) return
    try {
      const req = JSON.parse(line)
      const r = await prepareWithToken(req)
      process.stdout.write(JSON.stringify({ ok: true, url: r.url, body: r.body, headers: r.headers, uid: r.uid }) + '\n')
    } catch (e) {
      process.stdout.write(JSON.stringify({ ok: false, error: e.message }) + '\n')
    }
  })
  process.stdout.write(JSON.stringify({ ok: true, ready: true }) + '\n')
}
