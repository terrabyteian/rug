variable "bucket_name" {
  description = "S3 bucket name for the data lake"
  type        = string
  default     = "my-data-lake"
}

variable "region" {
  description = "AWS region"
  type        = string
  default     = "us-east-1"
}
