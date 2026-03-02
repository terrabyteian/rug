terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/services-verbose-deploy.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# Simulates a noisy deployment that produces enough output to require scrolling.
resource "null_resource" "verbose_deploy" {
  triggers = {
    deploy_id = var.deploy_id
  }

  provisioner "local-exec" {
    command = <<-EOF
      echo "=== Starting deployment: ${var.service_name} ==="
      echo "Resolving dependencies..."
      for i in $(seq 1 8); do
        echo "  dependency[$i]: hashicorp/null v3.2.4 -- ok"
      done
      echo ""
      echo "=== Pre-flight checks ==="
      for check in iam-permissions vpc-connectivity subnet-availability sg-rules kms-access s3-bucket ecr-repo lb-target-group; do
        echo "  [✓] $check"
      done
      echo ""
      echo "=== Building artifacts ==="
      for step in "compiling source" "running tests" "building image" "pushing to registry" "signing image" "generating SBOM" "uploading manifest"; do
        echo "  >> $step..."
        echo "     done"
      done
      echo ""
      echo "=== Deploying to cluster ==="
      for i in $(seq 1 6); do
        echo "  task[$i] registered"
        echo "  task[$i] pending"
        echo "  task[$i] running"
      done
      echo ""
      echo "=== Health checks ==="
      for i in $(seq 1 6); do
        echo "  task[$i] healthy (2/2 checks passing)"
      done
      echo ""
      echo "=== Traffic shifting ==="
      for pct in 10 25 50 75 100; do
        echo "  $pct% traffic → new version"
      done
      echo ""
      echo "=== Post-deploy validation ==="
      for endpoint in /health /ready /metrics /version; do
        echo "  GET ${var.service_name}$endpoint → 200 OK"
      done
      echo ""
      echo "=== Deployment complete ==="
      echo "  service : ${var.service_name}"
      echo "  deploy  : ${var.deploy_id}"
      echo "  tasks   : 6/6 running"
      echo "  elapsed : 42s"
    EOF
  }
}
