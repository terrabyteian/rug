terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/platform-dns.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# Sibling of networking/ under platform/ — also a root module.
resource "null_resource" "dns_zone" {
  triggers = {
    domain = var.domain
  }

  provisioner "local-exec" {
    command = "echo 'Configuring DNS zone: ${var.domain}'"
  }
}
