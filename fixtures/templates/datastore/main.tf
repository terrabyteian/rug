# Template skeleton for a managed datastore.
# No backend block: intentionally excluded from rug's root module list.

terraform {
  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# TODO: replace with RDS / DynamoDB / etc.
resource "null_resource" "placeholder" {}
