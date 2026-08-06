#!/usr/bin/env bash
# Homebrew helpers for setup.sh

ensure_cmd() {
  command -v "$1" &>/dev/null
}

ensure_brew_on_path() {
  if ensure_cmd brew; then
    return 0
  fi
  if [[ -x /opt/homebrew/bin/brew ]]; then
    eval "$(/opt/homebrew/bin/brew shellenv)"
  elif [[ -x /usr/local/bin/brew ]]; then
    eval "$(/usr/local/bin/brew shellenv)"
  fi
  ensure_cmd brew
}

install_homebrew() {
  if ensure_brew_on_path; then
    success "Homebrew already installed: $(brew --version | head -1)"
    return 0
  fi

  if [[ "${DRY_RUN:-0}" == "1" ]]; then
    info "[dry-run] Would install Homebrew"
    return 0
  fi

  info "Installing Homebrew..."
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  ensure_brew_on_path || die "Homebrew installed but not found on PATH"
  success "Homebrew installed"
}

ensure_brew_pkg() {
  local pkg="$1"
  if brew list --formula "$pkg" &>/dev/null || brew list --cask "$pkg" &>/dev/null; then
    return 0
  fi
  if [[ "${DRY_RUN:-0}" == "1" ]]; then
    info "[dry-run] Would brew install $pkg"
    return 0
  fi
  brew install "$pkg"
}

# Font casks are listed in the Brewfile for inventory / manual `brew bundle`,
# but setup installs them via lib/fonts.sh (graceful failure, --skip-fonts).
_brewfile_for_bundle() {
  local brewfile="$1"
  local filtered
  filtered="$(mktemp)"
  grep -v -E '^[[:space:]]*cask[[:space:]]+"font-' "$brewfile" >"$filtered" || true
  # Only the path on stdout — callers capture this via command substitution.
  printf '%s\n' "$filtered"
}

run_brew_bundle() {
  local brewfile="$1"
  local effective
  effective="$(_brewfile_for_bundle "$brewfile")"
  local -a args=(bundle --file="$effective" --no-lock)

  if [[ "${SKIP_CASKS:-0}" == "1" ]]; then
    args+=(--brews --taps)
  fi

  if [[ "${DRY_RUN:-0}" == "1" ]]; then
    info "[dry-run] Would run: brew ${args[*]}"
    brew bundle check --file="$effective" --no-upgrade 2>/dev/null || true
    rm -f "$effective"
    return 0
  fi

  info "Installing packages from Brewfile..."
  if brew "${args[@]}"; then
    rm -f "$effective"
    success "Brewfile applied"
  else
    local status=$?
    rm -f "$effective"
    return "$status"
  fi
}

ensure_rustup() {
  # Official ~/.cargo install or anything already on PATH
  if ensure_cmd rustup; then
    success "rustup already installed"
    return 0
  fi

  # Homebrew rustup is keg-only
  local brew_rustup_bin
  brew_rustup_bin="$(brew --prefix rustup 2>/dev/null)/bin"
  if [[ -x "${brew_rustup_bin}/rustup" ]]; then
    export PATH="${brew_rustup_bin}:${PATH}"
    success "rustup available via Homebrew"
    return 0
  fi

  if [[ "${DRY_RUN:-0}" == "1" ]]; then
    info "[dry-run] Would install rustup"
    return 0
  fi

  # Brewfile should have installed it; fall back to official installer
  info "Installing rustup via official installer..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
  # shellcheck disable=SC1091
  [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
  ensure_cmd rustup || die "rustup installation failed"
  success "rustup installed"
}
