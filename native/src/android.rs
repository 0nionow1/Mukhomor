use jni::{objects::{GlobalRef, JObject, JString, JValue}, JavaVM};
use std::sync::OnceLock;
use mukhomor_ui::slint;

static JVM: OnceLock<JavaVM> = OnceLock::new();
static BRIDGE: OnceLock<GlobalRef> = OnceLock::new();

pub fn call(method: &str, text: &str) -> Result<String,String> {
    let mut env=JVM.get().ok_or("Android VM is not ready")?.attach_current_thread().map_err(|e|e.to_string())?;
    let arg=env.new_string(text).map_err(|e|e.to_string())?;
    let result=env.call_static_method(BRIDGE.get().ok_or("Android bridge is not ready")?,method,"(Ljava/lang/String;)Ljava/lang/String;",&[JValue::Object(&arg)]);
    if env.exception_check().unwrap_or(false) { let _=env.exception_clear(); return Err("Android operation failed".into()); }
    let object=result.map_err(|e|e.to_string())?.l().map_err(|e|e.to_string())?;
    if object.is_null(){return Ok(String::new())};let result=JString::from(object);let value=env.get_string(&result).map_err(|e|e.to_string())?;Ok(value.into())
}
pub fn run(app: slint::android::AndroidApp) -> Result<(),String> {
    let vm=unsafe{JavaVM::from_raw(app.vm_as_ptr().cast())}.map_err(|e|e.to_string())?;
    {
        let mut env=vm.attach_current_thread().map_err(|e|e.to_string())?;
        let activity=unsafe{JObject::from_raw(app.activity_as_ptr().cast())};
        let class=env.call_method(&activity,"bridgeClass","()Ljava/lang/Class;",&[]).map_err(|e|e.to_string())?.l().map_err(|e|e.to_string())?;
        let global=env.new_global_ref(class).map_err(|e|e.to_string())?;
        let _=BRIDGE.set(global);
    }
    let _=JVM.set(vm);
    let root=call("dataPath", "")?;
    slint::android::init(app).map_err(|e|e.to_string())?;
    crate::slint_host::run(root.into())
}

pub fn prepare_launch(window: &slint::Window) -> Result<(), String> {
    let mut reported = false;
    window.set_rendering_notifier(move |state, _| {
        if matches!(state, slint::RenderingState::AfterRendering) && !reported {
            // The Android backend uses Skia. This callback follows the native
            // frame, unlike NativeActivity's earlier Java decor traversal.
            // AfterRendering precedes the buffer swap. Dispatch on the next
            // event-loop iteration so presentation has completed as well.
            reported = slint::invoke_from_event_loop(|| {
                if let Err(error) = call("firstFrame", "") {
                    eprintln!("Mukhomor launch transition: {error}");
                }
            }).is_ok();
        }
    }).map_err(|e| e.to_string())
}
