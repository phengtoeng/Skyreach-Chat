// Root Gradle for the native Android app. The Rust core ships as libseal_ffi.so
// in app/src/main/jniLibs/<abi>/ (build it with cargo-ndk — see README).
plugins {
    id("com.android.application") version "8.5.2" apply false
    id("org.jetbrains.kotlin.android") version "1.9.24" apply false
}
