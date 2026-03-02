terraform {
  required_version = ">= 0.12"

  # No backend block and no lock file — detected as root module because
  # terraform.tfstate exists on disk (old local-state workflow).

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

resource "null_resource" "legacy_app" {
  triggers = {
    version = var.app_version
  }

  provisioner "local-exec" {
    command = "echo 'Legacy monolith v${var.app_version}'"
  }
}
