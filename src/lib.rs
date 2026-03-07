use jni_simple::{
    JNI_OK, JNIEnv, JVMTI_ERROR_NONE, JVMTI_VERSION_1_2, JVMTIEnv, JavaVM, jclass, jint, jmethodID,
    jobject, jthread, jvmtiCapabilities, jvmtiError, jvmtiEvent, jvmtiEventCallbacks,
    jvmtiEventMode,
};

static BRIDGE_SIGNATURE: &str = "Lorg/dreamfinity/dsgl/core/HotReloadBridge;";
static MARK_METHOD: &str = "markHotSwap";
static MARK_METHOD_SIGNATURE: &str = "()V";
static VM_INIT_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static BRIDGE: std::sync::Mutex<Option<BridgeCache>> = std::sync::Mutex::new(None);

#[derive(Copy, Clone, Debug)]
struct BridgeCache {
    class_global: usize,
    mark_method: usize,
}

fn jvmti_ok(err: jvmtiError) -> bool {
    err == JVMTI_ERROR_NONE
}

fn clear_java_exception(env: &JNIEnv, context: &str) {
    unsafe {
        if env.ExceptionCheck() {
            eprintln!(
                "[dsgl-hotswap-agent] Java exception occurred in {}",
                context
            );
            env.ExceptionDescribe();
        }
    }
}

fn try_cache_bridge(jvmti: &JVMTIEnv, env: &JNIEnv, class: jclass) {
    unsafe {
        {
            match BRIDGE.lock() {
                Ok(mux) => {
                    if mux.is_some() {
                        return;
                    }
                }
                Err(err) => {
                    eprintln!("[dsgl-hotswap-agent] Mutex lock failed: {}", err);
                    return;
                }
            }
        }

        let mut signature_ptr: *mut std::ffi::c_char = std::ptr::null_mut();
        let err = jvmti.GetClassSignature(class, &mut signature_ptr, std::ptr::null_mut());

        if !jvmti_ok(err) {
            eprintln!("[dsgl-hotswap-agent] GetClassSignature failed: {}", err);
            return;
        }

        if signature_ptr.is_null() {
            eprintln!("[dsgl-hotswap-agent] GetClassSignature returned null");
            return;
        }

        let signature = std::ffi::CStr::from_ptr(signature_ptr)
            .to_string_lossy()
            .into_owned();

        let dealloc_err = jvmti.Deallocate(signature_ptr);
        if !jvmti_ok(dealloc_err) {
            eprintln!("[dsgl-hotswap-agent] Deallocate failed: {}", dealloc_err);
            return;
        }

        if signature.as_str() != BRIDGE_SIGNATURE {
            return;
        }

        let class_global = env.NewGlobalRef(class as jobject);
        if class_global.is_null() {
            clear_java_exception(env, "NewGlobalRef(HotReloadBridge)");
            eprintln!("[dsgl-hotswap-agent] NewGlobalRef failed");
            return;
        }

        let mark_method = env.GetStaticMethodID(
            class,
            MARK_METHOD.to_owned() + "\0",
            MARK_METHOD_SIGNATURE.to_owned() + "\0",
        );
        if mark_method.is_null() {
            clear_java_exception(
                env,
                &format!(
                    "GetStaticMethodID(HotReloadBridge, {}, {}",
                    MARK_METHOD, MARK_METHOD_SIGNATURE
                ),
            );
            eprintln!("[dsgl-hotswap-agent] GetMethodID failed");
            return;
        }

        {
            match BRIDGE.lock() {
                Ok(mut mux) => {
                    if mux.is_some() {
                        env.DeleteGlobalRef(class_global as jobject);
                        return;
                    }
                    *mux = Some(BridgeCache {
                        class_global: class_global as usize,
                        mark_method: mark_method as usize,
                    });
                    eprintln!("[dsgl-hotswap-agent] Bridge {:?} cached", mux.unwrap());
                }
                Err(err) => {
                    eprintln!("[dsgl-hotswap-agent] Mutex lock failed: {}", err);
                    return;
                }
            }
        }
    }
}

fn mark_hotswap_pending(env: &JNIEnv) -> Result<(), ()> {
    unsafe {
        let cache = match BRIDGE.lock() {
            Ok(mux) => {
                if mux.is_none() {
                    eprintln!("[dsgl-hotswap-agent] Bridge not cached - failed to mark HotSwap");
                    return Err(());
                }
                eprintln!("[dsgl-hotswap-agent] Bridge {:?} cached", mux.unwrap());
                mux.unwrap()
            }
            Err(err) => {
                eprintln!("[dsgl-hotswap-agent] Mutex lock failed: {}", err);
                return Err(());
            }
        };

        let class_global = cache.class_global as jclass;
        let mark_method = cache.mark_method as jmethodID;

        let saved_exception = if env.ExceptionCheck() {
            let ex = env.ExceptionOccurred();
            env.ExceptionClear();
            ex
        } else {
            std::ptr::null_mut()
        };
        eprintln!("[dsgl-hotswap-agent] Marking hotswap");
        env.CallStaticVoidMethod0(class_global, mark_method);

        if env.ExceptionCheck() {
            eprintln!("[dsgl-hotswap-agent] Exception occurred while marking hotswap");
            env.ExceptionDescribe();
        }

        if !saved_exception.is_null() {
            let throw_rc = env.Throw(saved_exception);
            if throw_rc != JNI_OK {
                eprintln!("[dsgl-hotswap-agent] Failed to throw exception");
            }
            env.DeleteLocalRef(saved_exception);
            return Err(());
        }
        eprintln!("[dsgl-hotswap-agent] HotSwap marked");
        Ok(())
    }
}

extern "system" fn vm_init(_jvmti_env: JVMTIEnv, _jni_env: JNIEnv, _thread: jthread) {
    VM_INIT_DONE.store(true, std::sync::atomic::Ordering::Release);
    println!("[dsgl-hotswap-agent] VM init");
}

extern "system" fn class_prepare(jvmti: JVMTIEnv, env: JNIEnv, _thread: jthread, class: jclass) {
    try_cache_bridge(&jvmti, &env, class);
}

extern "system" fn class_load_hook(
    jvmti: JVMTIEnv,
    env: JNIEnv,
    class_being_redefined: jclass,
    _loader: jobject,
    name: *const std::ffi::c_char,
    _protection_domain: jobject,
    _class_data_len: jint,
    _class_data: *const std::ffi::c_uchar,
    _new_class_data_len: *mut jint,
    _new_class_data: *mut *mut std::ffi::c_uchar,
) {
    unsafe {
        if class_being_redefined.is_null() {
            return;
        }

        if !VM_INIT_DONE.load(std::sync::atomic::Ordering::Acquire) {
            eprintln!("[dsgl-hotswap-agent] VM not initialized");
            return;
        }

        let class_name = if name.is_null() {
            eprintln!("[dsgl-hotswap-agent] class_name is null");
            "<unnamed>".to_owned()
        } else {
            std::ffi::CStr::from_ptr(name)
                .to_string_lossy()
                .into_owned()
        };

        eprintln!(
            "[dsgl-hotswap-agent] HotSwap detected for class {}",
            class_name
        );

        match mark_hotswap_pending(&env) {
            Ok(_) => (),
            Err(_) => {
                eprintln!("[dsgl-hotswap-agent] Trying to cache bridge recursively");
                try_cache_bridge(&jvmti, &env, class_being_redefined);
                mark_hotswap_pending(&env);
            }
        };
    }
}

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub extern "system" fn Agent_OnLoad(
    vm: JavaVM,
    _options: *const char,
    _reserved: *mut std::os::raw::c_void,
) -> jint {
    unsafe {
        let jvmti = match vm.GetEnv::<JVMTIEnv>(JVMTI_VERSION_1_2) {
            Ok(jvmti) => jvmti,
            Err(err) => {
                eprintln!(
                    "[dsgl-hotswap-agent] Failed to get JVMTI environment: {}",
                    err
                );
                return -1;
            }
        };

        let mut capabilities = jvmtiCapabilities::default();
        capabilities.set_can_generate_all_class_hook_events(true);
        let err = jvmti.AddCapabilities(&capabilities);
        if !jvmti_ok(err) {
            eprintln!("[dsgl-hotswap-agent] Failed to add capabilities: {}", err);
            return -1;
        }

        let mut callbacks = jvmtiEventCallbacks::default();
        callbacks.VMInit = Some(vm_init);
        callbacks.ClassPrepare = Some(class_prepare);
        callbacks.ClassFileLoadHook = Some(class_load_hook);

        let err = jvmti.SetEventCallbacks(&callbacks);
        if !jvmti_ok(err) {
            eprintln!(
                "[dsgl-hotswap-agent] Failed to set event callbacks: {}",
                err
            );
            return -1;
        }

        let err = jvmti.SetEventNotificationMode(
            jvmtiEventMode::JVMTI_ENABLE,
            std::mem::transmute(50), // workaround - currently jni-simple doesn't have JVMTI_EVENT_VM_INIT value for unknown reason
            std::ptr::null_mut(),
        );
        if !jvmti_ok(err) {
            eprintln!(
                "[dsgl-hotswap-agent] Failed to enable VM_START event: {}",
                err
            );
            return -1;
        }

        let err = jvmti.SetEventNotificationMode(
            jvmtiEventMode::JVMTI_ENABLE,
            jvmtiEvent::JVMTI_EVENT_CLASS_PREPARE,
            std::ptr::null_mut(),
        );
        if !jvmti_ok(err) {
            eprintln!(
                "[dsgl-hotswap-agent] Failed to enable CLASS_PREPARE event: {}",
                err
            );
        }

        let err = jvmti.SetEventNotificationMode(
            jvmtiEventMode::JVMTI_ENABLE,
            jvmtiEvent::JVMTI_EVENT_CLASS_FILE_LOAD_HOOK,
            std::ptr::null_mut(),
        );
        if !jvmti_ok(err) {
            eprintln!(
                "[dsgl-hotswap-agent] Failed to enable CLASS_FILE_LOAD_HOOK event: {}",
                err
            );
        }

        eprintln!("[dsgl-hotswap-agent] HotSwap agent loaded");

        JNI_OK
    }
}
