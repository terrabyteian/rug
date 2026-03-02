# Fixture: all-operations
#
# Produces every resource operation type that appears in the plan/apply
# summary line, so all count symbols are visible in the rug task list at once:
#
#   +1 add     null_resource.to_add         (new, absent from pre-seeded state)
#   ~1 change  terraform_data.to_change     (input differs from pre-seeded state)
#   -1 destroy null_resource.to_destroy     (in state, removed from config)
#   i1 import  terraform_data.to_import     (external id brought into state)
#   f1 forget  null_resource.to_forget      (dropped from state, not destroyed)
#
# NOTE: `moved` blocks are included below for completeness but OpenTofu does
# not count moves in the Plan:/Apply complete! summary line — they only appear
# in the plan body text.  The >N move symbol will not be shown by rug.
#
# State is pre-seeded and committed alongside this file.  After an apply,
# reset with:  git checkout fixtures/services/all-operations/terraform.tfstate
#
# Requires: OpenTofu >= 1.7  (removed { lifecycle { destroy = false } })
# Run `init` (i key in rug) before first use.

terraform {
  required_version = ">= 1.0"

  # Store state next to the config so the pre-seeded file is always used.
  backend "local" {
    path = "terraform.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# ── ADD ───────────────────────────────────────────────────────────────────────
# Not present in pre-seeded state → tofu plans to create it.
resource "null_resource" "to_add" {}

# ── CHANGE ────────────────────────────────────────────────────────────────────
# terraform_data is a built-in resource (no provider declaration needed).
# Pre-seeded state has input = "old_value"; config says "new_value" → in-place
# update (not a replacement — only triggers_replace causes recreation).
resource "terraform_data" "to_change" {
  input = "new_value"
}

# ── DESTROY ───────────────────────────────────────────────────────────────────
# null_resource.to_destroy exists in pre-seeded state but is not declared
# anywhere in this config → tofu plans to delete it.

# ── IMPORT ────────────────────────────────────────────────────────────────────
# terraform_data supports import; null_resource does not (as of null ~3.0).
# The resource is absent from pre-seeded state so this shows as "to import".
import {
  to = terraform_data.to_import
  id = "rug-fixture-import-id"
}

resource "terraform_data" "to_import" {}

# ── FORGET (OpenTofu >= 1.7) ──────────────────────────────────────────────────
# null_resource.to_forget exists in pre-seeded state; this block removes it
# from state only — destroy = false means the actual resource is left untouched.
removed {
  from = null_resource.to_forget
  lifecycle {
    destroy = false
  }
}

# ── MOVE (body-only, not counted in summary) ──────────────────────────────────
# null_resource.moved_source exists in state; this renames it in-place.
# OpenTofu shows this in the plan body but does not include it in the
# "Plan: N to add..." summary line, so rug cannot display it as a count.
moved {
  from = null_resource.moved_source
  to   = null_resource.moved_target
}

resource "null_resource" "moved_target" {}
