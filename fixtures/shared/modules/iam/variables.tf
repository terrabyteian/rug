variable "role_name" {
  description = "Name for the IAM role"
  type        = string
}

variable "policy_arns" {
  description = "List of policy ARNs to attach"
  type        = list(string)
  default     = []
}
