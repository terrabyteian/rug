variable "db_name" {
  description = "Database name (fill in before use)"
  type        = string
  default     = "mydb"
}

variable "engine" {
  description = "Database engine"
  type        = string
  default     = "postgres"
}
