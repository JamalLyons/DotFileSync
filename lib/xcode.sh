#!/usr/bin/env bash
# Install / select the latest Xcode via xcodes
# https://github.com/XcodesOrg/xcodes

setup_xcode() {
  if [[ "${SKIP_XCODE:-0}" == "1" ]]; then
    info "Skipping Xcode install (--skip-xcode)"
    return 0
  fi

  ensure_brew_on_path || true

  if ! ensure_cmd xcodes; then
    if [[ "${DRY_RUN:-0}" == "1" ]]; then
      info "[dry-run] Would install xcodes and latest Xcode"
      return 0
    fi
    warning "xcodes not found; installing via Homebrew..."
    brew install xcodes || {
      warning "Could not install xcodes; skipping Xcode setup"
      return 0
    }
  fi

  if [[ "${DRY_RUN:-0}" == "1" ]]; then
    info "[dry-run] Would run: xcodes install --latest --select"
    if [[ -d /Applications/Xcode.app ]]; then
      info "[dry-run] Xcode already present: $(xcodebuild -version 2>/dev/null | head -1 || echo unknown)"
    fi
    return 0
  fi

  # Command Line Tools (needed even when full Xcode is present on some machines)
  if ! xcode-select -p &>/dev/null; then
    info "Xcode Command Line Tools missing; triggering installer (GUI may appear)..."
    xcode-select --install 2>/dev/null || true
  fi

  info "Installing latest Xcode via xcodes (Apple ID sign-in may be required)..."
  info "This can take a long time and several GB of disk space."

  local -a install_args=(install --latest --select --update --experimental-unxip --empty-trash)
  if ensure_cmd aria2c; then
    info "Using aria2 for a faster Xcode download"
  else
    install_args+=(--no-aria2)
  fi

  if xcodes "${install_args[@]}"; then
    _xcode_post_install
    success "Xcode ready: $(xcodebuild -version 2>/dev/null | tr '\n' ' ')"
  else
    if [[ -d /Applications/Xcode.app ]] && xcodebuild -version &>/dev/null; then
      warning "xcodes could not install/update latest, but an existing Xcode is usable"
      _xcode_post_install
      success "Using existing Xcode: $(xcodebuild -version 2>/dev/null | tr '\n' ' ')"
    else
      warning "Xcode install failed or was cancelled. Sign in with an Apple ID and re-run:"
      warning "  xcodes install --latest --select"
    fi
  fi
}

_xcode_post_install() {
  local developer_dir="/Applications/Xcode.app/Contents/Developer"
  if [[ -d "$developer_dir" ]]; then
    if [[ "$(xcode-select -p 2>/dev/null || true)" != "$developer_dir" ]]; then
      info "Selecting Xcode developer directory (may ask for password)..."
      sudo xcode-select -s "$developer_dir" 2>/dev/null || true
    fi
  fi

  # Accept license + finish first-launch packages non-interactively when possible
  sudo xcodebuild -license accept 2>/dev/null || true
  sudo xcodebuild -runFirstLaunch 2>/dev/null || true
}
