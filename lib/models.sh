#!/usr/bin/env bash
# Install preferred local models via LM Studio CLI (lms get)

install_models() {
  local models=(
    "prism-ml/bonsai-27b"
    "qwen/qwen3-1.7b"
    "qwen/qwen3-4b-2507"
  )

  local lms_bin="lms"
  if ! command -v lms &>/dev/null; then
    if [[ -x "${HOME}/.lmstudio/bin/lms" ]]; then
      lms_bin="${HOME}/.lmstudio/bin/lms"
    elif [[ -x "${HOME}/.cache/lm-studio/bin/lms" ]]; then
      lms_bin="${HOME}/.cache/lm-studio/bin/lms"
    else
      warning "LM Studio CLI (lms) not found; skipping model installation."
      return 0
    fi
  fi

  info "Installing LM Studio models..."
  local model
  for model in "${models[@]}"; do
    if [[ "${DRY_RUN:-0}" == "1" ]]; then
      info "[dry-run] Would run: ${lms_bin} get ${model}"
      continue
    fi
    "${lms_bin}" get "${model}" --mlx -y || warning "Failed to install model: ${model}"
  done
  success "LM Studio models installed"
}
