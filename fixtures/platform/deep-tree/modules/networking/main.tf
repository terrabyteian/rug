# Library module — no backend block, no state file (excluded by default).
# Level 1 of the deep-tree nesting (root -> networking -> subnets -> routes).

resource "null_resource" "vpc" {}

resource "null_resource" "gateway" {
  count = 2
}

# Data source inside a module — exercises the explorer's data-address
# handling (data resources are skipped by taint/untaint).
data "null_data_source" "networking_info" {}

module "subnets" {
  source = "./subnets"
}
