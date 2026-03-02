terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/data-lake.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

resource "null_resource" "lake" {
  triggers = {
    bucket = var.bucket_name
    region = var.region
  }

  provisioner "local-exec" {
    command = "echo 'Data lake: s3://${var.bucket_name} in ${var.region}'"
  }
}
