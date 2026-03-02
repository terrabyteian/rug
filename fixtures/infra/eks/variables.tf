variable "cluster_name" {
  description = "Name of the EKS cluster"
  type        = string
  default     = "main-cluster"
}

variable "node_count" {
  description = "Number of worker nodes"
  type        = number
  default     = 3
}
