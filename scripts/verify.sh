#!/usr/bin/env bash
# Runs all checks of a work package in one go: formatting, Clippy, tests, app build, UI previews, Clippy for
# Android (where an SDK is installed) and the skill check. Every step runs even when an earlier one fails; the exit code is 1 if any step failed.
# Linux only. Usage (repo root): scripts/verify.sh [--snapshot[=<scenes>]]
#   --snapshot           also renders all UI snapshot scenes against a fake Moonraker (about 1.5 minutes more); once
#                        before a commit
#   --snapshot=pages,..  only these scenes, for the part of the UI that changed (names: scripts/ui-snapshot.sh -h)
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1
log_dir=$(mktemp -d "${TMPDIR:-/tmp}/sailhand-verify.XXXXXX")
with_snapshot=false
snapshot_arguments=()
case "${1:-}" in
    --snapshot) with_snapshot=true ;;
    --snapshot=*) with_snapshot=true; snapshot_arguments=(-S "${1#--snapshot=}") ;;
esac
failed=()

run_step() {
    local name=$1
    shift
    local log="$log_dir/${name// /_}.log"
    local start=$SECONDS
    if "$@" > "$log" 2>&1; then
        echo "OK    $name ($((SECONDS - start)) s)"
    else
        echo "FAIL  $name after $((SECONDS - start)) s (last lines in $log)"
        tail -n 15 "$log" | sed 's/^/        /'
        failed+=("$name")
    fi
}

run_step "cargo fmt --check" cargo fmt --all -- --check
run_step "cargo clippy" cargo clippy --workspace --all-targets -- -D warnings
run_step "cargo test" cargo test --workspace
# The whole workspace, not only -p sailhand: the same features as the test run, so the UI crate is not compiled again.
run_step "cargo build" cargo build --workspace --bins
run_step "ui previews" scripts/ui-preview.sh

# Clippy for Android as in CI: code behind `cfg(target_os = "android")` is not compiled on the desktop (#239 broke
# only there). Needs cargo-ndk, the Rust target and an SDK with the NDK of android/app/build.gradle.kts.
android_clippy() {
    local sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}
    local ndk_version jar
    ndk_version=$(sed -n 's/^ *ndkVersion = "\(.*\)"/\1/p' android/app/build.gradle.kts)
    # Slint's build script finds no platform by itself, so the newest android.jar is passed explicitly.
    jar=$(find "$sdk/platforms" -maxdepth 2 -name android.jar 2>/dev/null | sort -V | tail -n 1)
    ANDROID_NDK_HOME=$sdk/ndk/$ndk_version ANDROID_JAR=$jar \
        cargo ndk -t arm64-v8a clippy -p sailhand-android -- -D warnings
}
android_sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}
if cargo ndk --version > /dev/null 2>&1 && [ -d "$android_sdk/ndk" ] \
    && rustup target list --installed 2>/dev/null | grep -qx aarch64-linux-android; then
    run_step "cargo clippy android" android_clippy
else
    echo "SKIP  cargo clippy android (needs cargo-ndk, rustup target aarch64-linux-android and ANDROID_HOME)"
fi
# The AI skills are local (.claude/ is not in the repository); check them only where they exist.
skill_check=.claude/skills/session-handover/scripts/check-skills.sh
if [ -x "$skill_check" ]; then
    run_step "skill check" "$skill_check"
fi
if [ "$with_snapshot" = true ]; then
    run_step "ui snapshot" scripts/ui-snapshot.sh ${snapshot_arguments[@]+"${snapshot_arguments[@]}"}
fi

echo
if [ "${#failed[@]}" -eq 0 ]; then
    echo "All checks passed. Logs: $log_dir"
    exit 0
fi
echo "Failed: ${failed[*]}. Logs: $log_dir"
exit 1
