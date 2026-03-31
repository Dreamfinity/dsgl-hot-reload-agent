# DSGL Hot Reload Agent

Native JVMTI agent for DSGL hot reload.

This subproject is written in Rust and built as a `cdylib`. It is loaded into the JVM and watches for class redefinition events. When that happens, it calls `org.dreamfinity.dsgl.core.HotReloadBridge.markHotSwap()`, so DSGL knows it needs to rebuild the UI.

> This project is **not** part of the main Gradle multi-module build (`:core`, `:mc1710`, `:mc1710-demo`). It is built separately with Cargo.

## Purpose

The agent does **not** perform hot swap itself and does **not** patch bytecode. Its job is only to detect that the JVM has redefined a class and notify DSGL.

On the DSGL side, `DsglScreenHost` checks `HotReloadBridge.consumeHotSwap()`. If the flag is set, it rebuilds the retained UI tree.

## Relevant Files

- Agent: `dsgl-hot-reload-agent/src/lib.rs`
- Bridge flag: `core/src/main/kotlin/org/dreamfinity/dsgl/core/HotReloadBridge.kt`
- Rebuild trigger: `mc1710/src/main/kotlin/org/dreamfinity/dsgl/mc1710/DsglScreenHost.kt`

## How It Works

At startup, `Agent_OnLoad` acquires the JVMTI environment (`JVMTI_VERSION_1_2`) and enables the events the agent needs.

The agent listens to:

- `VMInit`
- `ClassPrepare`
- `ClassFileLoadHook`

When `HotReloadBridge` is prepared, the agent caches:

- a global reference to `org/dreamfinity/dsgl/core/HotReloadBridge`
- the static method ID for `markHotSwap(): void`

Later, when `ClassFileLoadHook` is invoked for a class redefinition (`class_being_redefined != null`), the agent calls the cached `markHotSwap()` method.

If the bridge has not been cached yet, the agent tries to resolve it again on the first redefine path.

## Build

### From repository root

```bash
cargo build --manifest-path dsgl-hot-reload-agent/Cargo.toml
```

The produced library name is OS-specific:

- Windows: `dsgl_hot_reload_agent.dll`
- Linux: `libdsgl_hot_reload_agent.so`
- macOS: `libdsgl_hot_reload_agent.dylib`
