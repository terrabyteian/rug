terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/apps-api.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

resource "null_resource" "api_service" {
  triggers = {
    service_name = var.service_name
    image_tag    = var.image_tag
  }

  provisioner "local-exec" {
    command = "echo 'Deploying API: ${var.service_name}:${var.image_tag}'"
  }
}
