# Template skeleton — fill in before deploying.
# No backend block: intentionally excluded from rug's root module list.

terraform {
  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# TODO: replace with real resources
resource "null_resource" "placeholder" {}
