#!/bin/sh

deploy_step() {
  [ "$#" -ge 2 ] || {
    echo "deploy_step requires a step name and command" >&2
    return 64
  }
  step_name=$1
  shift

  if [ "${BRUNN_DEPLOY_FAIL_STEP:-}" = "$step_name" ]; then
    echo "deployment step failed by fault injection: $step_name" >&2
    return 97
  fi
  if "$@"; then
    return 0
  else
    status=$?
    echo "deployment step failed: $step_name (exit $status)" >&2
    return "$status"
  fi
}
