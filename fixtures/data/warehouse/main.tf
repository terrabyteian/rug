terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/data-warehouse.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

resource "null_resource" "warehouse" {
  triggers = {
    schema  = var.schema
    cluster = var.cluster
  }

  provisioner "local-exec" {
    command = "echo 'Data warehouse: ${var.cluster}/${var.schema}'"
  }
}
