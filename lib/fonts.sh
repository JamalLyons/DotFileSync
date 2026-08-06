#!/usr/bin/env bash
# Preferred coding fonts via Homebrew font casks
# https://github.com/0xType/0xProto

# Pin preference to the release the user linked; bump when upgrading.
# Homebrew's font-0xproto cask currently ships this version.
OXPROTO_PREFERRED_VERSION="2.502"
OXPROTO_CASK="font-0xproto"

# Marker files Homebrew / Font Book install under ~/Library/Fonts
OXPROTO_FONT_MARKERS=(
  "0xProto-Regular.otf"
  "0xProto-Bold.otf"
  "0xProto-Italic.otf"
)

oxproto_fonts_present() {
  local font_dir="${HOME}/Library/Fonts"
  local name
  for name in "${OXPROTO_FONT_MARKERS[@]}"; do
    if [[ ! -f "${font_dir}/${name}" ]]; then
      return 1
    fi
  done
  return 0
}

install_fonts() {
  if [[ "${SKIP_FONTS:-0}" == "1" ]]; then
    info "Skipping fonts (--skip-fonts)"
    return 0
  fi

  ensure_brew_on_path || true

  if oxproto_fonts_present || brew list --cask "${OXPROTO_CASK}" &>/dev/null; then
    success "0xProto already installed (prefer ${OXPROTO_PREFERRED_VERSION})"
    return 0
  fi

  if [[ "${DRY_RUN:-0}" == "1" ]]; then
    info "[dry-run] Would brew install --cask ${OXPROTO_CASK} (prefer ${OXPROTO_PREFERRED_VERSION})"
    return 0
  fi

  if ! ensure_cmd brew; then
    warning "Homebrew not available; skipping 0xProto font install"
    return 0
  fi

  info "Installing 0xProto via Homebrew (${OXPROTO_CASK}, prefer ${OXPROTO_PREFERRED_VERSION})..."
  if brew install --cask "${OXPROTO_CASK}"; then
    if oxproto_fonts_present || brew list --cask "${OXPROTO_CASK}" &>/dev/null; then
      success "0xProto fonts installed"
    else
      warning "brew reported success but 0xProto font files were not found in ~/Library/Fonts"
    fi
  else
    warning "Could not install ${OXPROTO_CASK} (network or brew issue). Continuing without it."
    warning "Re-run later: brew install --cask ${OXPROTO_CASK}"
    warning "Release: https://github.com/0xType/0xProto/releases/tag/${OXPROTO_PREFERRED_VERSION}"
  fi
}
