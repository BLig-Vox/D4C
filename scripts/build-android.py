#!/usr/bin/env python3
"""Build a native arm64 preview APK without Gradle, under the project RAM guard."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SDK = Path(os.environ.get('ANDROID_HOME', Path.home() / 'Android/Sdk'))
NDK = Path(os.environ.get('ANDROID_NDK_HOME', SDK / 'ndk/28.2.13676358'))
TOOLS = SDK / 'build-tools/35.0.0'
TARGET = ROOT / 'target/android-build'
OUT = ROOT / 'target/android'

def call(args, **kwargs):
    subprocess.run([str(a) for a in args], check=True, **kwargs)

def worker():
    clang = NDK / 'toolchains/llvm/prebuilt/linux-x86_64/bin'
    env = os.environ.copy()
    env.update(ANDROID_HOME=str(SDK), ANDROID_NDK_HOME=str(NDK),
        CARGO_TARGET_DIR=str(TARGET), CARGO_BUILD_JOBS='1',
        CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=str(clang / 'aarch64-linux-android23-clang'),
        CC_aarch64_linux_android=str(clang / 'aarch64-linux-android23-clang'),
        AR_aarch64_linux_android=str(clang / 'llvm-ar'),
        CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS='-C link-arg=-Wl,--threads=1 -C link-arg=-Wl,-z,max-page-size=16384',
        JAVA_TOOL_OPTIONS='-Xmx256m')
    for path in [clang / 'aarch64-linux-android23-clang', TOOLS / 'aapt', SDK / 'platforms/android-35/android.jar']:
        if not path.exists():
            raise SystemExit(f'Missing Android build tool: {path}')
    if '--package-only' not in sys.argv:
        call(['cargo', 'build', '--manifest-path', ROOT / 'Cargo.toml', '--locked', '--lib', '--target', 'aarch64-linux-android', '-j', '1'], env=env)
    OUT.mkdir(parents=True, exist_ok=True)
    classes = OUT / 'classes'
    dex = OUT / 'dex'
    classes.mkdir(exist_ok=True)
    dex.mkdir(exist_ok=True)
    android_jar = SDK / 'platforms/android-35/android.jar'
    java_sources = sorted((ROOT / 'android/java').rglob('*.java'))
    call(['javac', '-J-Xmx256m', '--release', '8', '-classpath', android_jar, '-d', classes, *java_sources], env=env)
    call([TOOLS / 'd8', '-JXmx256m', '--lib', android_jar, '--min-api', '23', '--output', dex, *sorted(classes.rglob('*.class'))], env=env)
    unsigned = OUT / 'd4c-unsigned.apk'
    aligned = OUT / 'd4c-aligned.apk'
    apk = OUT / 'd4c-arm64-preview.apk'
    library = OUT / 'libvox_d4c.so'
    shutil.copy2(TARGET / 'aarch64-linux-android/debug/libvox_d4c.so', library)
    call([clang / 'llvm-strip', '--strip-debug', library])
    call([TOOLS / 'aapt', 'package', '-f', '-M', ROOT / 'android/AndroidManifest.xml', '-S', ROOT / 'android/res', '-A', ROOT / 'assets/android-fonts', '-I', SDK / 'platforms/android-35/android.jar', '-F', unsigned])
    with zipfile.ZipFile(unsigned, 'a', compression=zipfile.ZIP_STORED) as archive:
        archive.write(dex / 'classes.dex', 'classes.dex')
        archive.write(library, 'lib/arm64-v8a/libvox_d4c.so')
        dependencies = subprocess.check_output([str(clang / 'llvm-readelf'), '-d', str(library)], text=True)
        if 'libc++_shared.so' in dependencies:
            archive.write(clang.parent / 'sysroot/usr/lib/aarch64-linux-android/libc++_shared.so', 'lib/arm64-v8a/libc++_shared.so')
    call([TOOLS / 'zipalign', '-P', '16', '-f', '4', unsigned, aligned])
    keystore = Path.home() / '.android/debug.keystore'
    if not keystore.exists():
        keystore.parent.mkdir(parents=True, exist_ok=True)
        call(['keytool', '-genkeypair', '-keystore', keystore, '-storepass', 'android', '-alias', 'androiddebugkey', '-keypass', 'android', '-keyalg', 'RSA', '-keysize', '2048', '-validity', '10000', '-dname', 'CN=Android Debug,O=Android,C=US'], env=env)
        keystore.chmod(0o600)
    call([TOOLS / 'apksigner', 'sign', '--ks', keystore, '--ks-key-alias', 'androiddebugkey', '--ks-pass', 'pass:android', '--key-pass', 'pass:android', '--out', apk, aligned], env=env)
    call([TOOLS / 'apksigner', 'verify', apk], env=env)
    call([TOOLS / 'zipalign', '-c', '-P', '16', '4', apk])
    print(f'APK ready: {apk}', flush=True)

if __name__ == '__main__':
    if '--worker' in sys.argv:
        worker()
    else:
        guard = Path(os.environ.get('D4C_BUILD_GUARD', ROOT / 'scripts/build-limited.py'))
        if not guard.exists():
            raise SystemExit('Set D4C_BUILD_GUARD to the memory-capped build supervisor')
        call([sys.executable, '-I', guard, '--limit-mb', '2000', '--report', ROOT / 'target/android-build-limit.json', '--', 'nice', '-n', '10', sys.executable, '-I', Path(__file__).resolve(), '--worker', *sys.argv[1:]])
