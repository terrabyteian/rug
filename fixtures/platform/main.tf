terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../.state/platform.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# Top-level platform module — an intermediate parent that is itself deployable.
# Its children (networking/, dns/) are also independent root modules.
resource "null_resource" "platform" {
  triggers = {
    env = var.environment
  }

  provisioner "local-exec" {
    command = "echo 'Configuring platform for env: ${var.environment}'"
  }
}
