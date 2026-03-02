terraform {
  required_version = ">= 1.0"

  # Has a backend block despite living under infra/modules/ — rug detects it
  # as a root module because classification is based on signals, not path.
  backend "local" {
    path = "../../../.state/infra-modules-deployable-db.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

resource "null_resource" "database" {
  triggers = {
    engine  = var.engine
    db_name = var.db_name
  }

  provisioner "local-exec" {
    command = "echo 'Provisioning DB: ${var.engine}/${var.db_name}'"
  }
}
