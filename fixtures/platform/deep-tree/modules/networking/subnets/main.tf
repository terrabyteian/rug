# Library module — level 2 of nesting (networking -> subnets -> routes).

resource "null_resource" "subnet_a" {}

resource "null_resource" "subnet_b" {}

module "routes" {
  source = "./routes"
}
