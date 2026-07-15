# Library module — level 3 (leaf) of nesting.
# null_resource.default_route is tainted after apply as the fixture's live
# [tainted] example (see main.tf header for the exact taint command).

resource "null_resource" "default_route" {}

resource "null_resource" "static_route" {
  count = 2
}
