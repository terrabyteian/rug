terraform {
  required_version = ">= 1.0"

  # No backend block here — this module is detected as a root module
  # solely because .terraform.lock.hcl exists (initialized but backend
  # configured elsewhere, e.g. via TF_BACKEND_CONFIG).

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

resource "null_resource" "api_gateway" {
  triggers = {
    stage = var.stage
  }

  provisioner "local-exec" {
    command = "echo 'Deploying API Gateway: stage=${var.stage}'"
  }
}
