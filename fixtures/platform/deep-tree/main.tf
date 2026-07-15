# Fixture: deep-tree
#
# Deep module nesting (3 levels: networking -> subnets -> routes) plus an
# indexed module instance with a dotted for_each key (module.cell["us.east-1"])
# to exercise the state explorer's grouped module rows and whole-module
# targeting (-target=module.<addr>), including bracket-aware address parsing
# for keys that themselves contain a dot.
#
# Uses default local state (terraform.tfstate committed in-directory, like
# fixtures/legacy/monolith and fixtures/services/all-operations).
#
# After `tofu init && tofu apply`, one nested resource is tainted so the
# explorer's [tainted] tag has a live example:
#   tofu taint 'module.networking.module.subnets.module.routes.null_resource.default_route'
#
# Run `init` (i key in rug) before first use.

terraform {
  required_version = ">= 1.0"

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# ── Root-level resources ─────────────────────────────────────────────────────

resource "null_resource" "root_seed" {}

resource "null_resource" "root_worker" {
  count = 2
}

resource "terraform_data" "root_tag" {
  for_each = toset(["blue", "green"])
  input    = each.key
}

# Always-diff resource: input = timestamp() changes on every plan, so there is
# always at least one change to demo targeted plan/apply against.
resource "terraform_data" "drift_probe" {
  input = timestamp()
}

# ── Deep nesting (3 levels): networking -> subnets -> routes ────────────────

module "networking" {
  source = "./modules/networking"
}

# ── Indexed module instances with a dotted for_each key ─────────────────────
# "us.east-1" contains a literal dot inside the key itself, producing the
# state address module.cell["us.east-1"].* — exercises bracket-aware parsing
# that must not split on the dot inside the quoted key.

module "cell" {
  source   = "./modules/cell"
  for_each = toset(["us.east-1", "eu-west-1"])
}
