terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/apps-web.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

resource "null_resource" "web_service" {
  triggers = {
    service_name = var.service_name
  }

  provisioner "local-exec" {
    command = "echo 'Deploying web: ${var.service_name}'"
  }
}
