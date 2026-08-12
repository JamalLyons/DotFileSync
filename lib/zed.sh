#!/usr/bin/env bash
# Zed editor config sync from checked-in templates

REPO_ZED_DIR="${REPO_ROOT}/zed"
ZED_CONFIG_DIR="${HOME}/.config/zed"

# Copy a single template into the live Zed config dir.
# Quiet install when missing; skip when identical; prompt before overwrite when different.
install_zed_file() {
  local src="$1"
  local dest="$2"
  local label
  label="$(basename "$src")"

  if [[ ! -f "$src" ]]; then
    return 0
  fi

  if [[ ! -f "$dest" ]]; then
    if [[ "${DRY_RUN:-0}" == "1" ]]; then
      info "[dry-run] Would install $label → $dest"
      return 0
    fi
    mkdir -p "$(dirname "$dest")"
    cp "$src" "$dest"
    success "Installed Zed $label → $dest"
    return 0
  fi

  if cmp -s "$src" "$dest"; then
    success "Zed $label already in sync"
    return 0
  fi

  if [[ "${DRY_RUN:-0}" == "1" ]]; then
    warning "[dry-run] Existing Zed $label differs from template (would prompt to overwrite)"
    info "[dry-run] Would leave $dest unchanged unless overwrite confirmed"
    return 0
  fi

  warning "Existing Zed config found (differs from template): $dest"
  info "Template: $src"
  local reply=""
  # Default N — safer than clobbering a customized machine config
  read -r -p "Overwrite? [y/N] " reply || true
  case "$reply" in
    y | Y)
      cp "$src" "$dest"
      success "Overwrote Zed $label → $dest"
      ;;
    *)
      info "Kept existing Zed $label"
      ;;
  esac
}

sync_zed_config() {
  if [[ "${SKIP_ZED:-0}" == "1" ]]; then
    info "Skipping Zed config (--skip-zed)"
    return 0
  fi

  info "Syncing Zed configuration..."

  if [[ ! -d "$REPO_ZED_DIR" ]]; then
    warning "Missing Zed templates at $REPO_ZED_DIR; skipping"
    return 0
  fi

  if [[ ! -f "$REPO_ZED_DIR/settings.json" ]]; then
    warning "No zed/settings.json template; skipping"
    return 0
  fi

  if [[ "${DRY_RUN:-0}" != "1" ]]; then
    mkdir -p "$ZED_CONFIG_DIR"
  else
    info "[dry-run] Would ensure directory exists: $ZED_CONFIG_DIR"
  fi

  install_zed_file "$REPO_ZED_DIR/settings.json" "$ZED_CONFIG_DIR/settings.json"

  # Optional — only when a checked-in keymap template exists
  if [[ -f "$REPO_ZED_DIR/keymap.json" ]]; then
    install_zed_file "$REPO_ZED_DIR/keymap.json" "$ZED_CONFIG_DIR/keymap.json"
  fi

  success "Zed configuration sync complete"
}
