variable "aws_region" {
  description = "AWS region for the deployment. us-east-1 is the low-cost default for validation."
  type        = string
  default     = "us-east-1"
}

variable "name" {
  description = "Name prefix for AWS resources."
  type        = string
  default     = "flowplane"
}

variable "environment" {
  description = "Environment label used in tags and names."
  type        = string
  default     = "prod-smoke"
}

variable "tags" {
  description = "Additional tags to apply to resources."
  type        = map(string)
  default     = {}
}

variable "vpc_cidr" {
  description = "CIDR for the deployment VPC."
  type        = string
  default     = "10.42.0.0/16"
}

variable "availability_zones" {
  description = "Availability zones to use. Keep these in aws_region; two is the minimum for ALB/RDS subnet groups."
  type        = list(string)
  default     = ["us-east-1a", "us-east-1b"]

  validation {
    condition     = length(var.availability_zones) >= 2
    error_message = "availability_zones must contain at least two AZ names."
  }
}

variable "enable_nat_gateway" {
  description = "Create NAT egress for private ECS tasks. Required for external OIDC/JWKS providers such as Auth0 unless another egress path exists."
  type        = bool
  default     = true
}

variable "single_nat_gateway" {
  description = "Use one NAT gateway for the smoke environment. Set false for one NAT gateway per AZ."
  type        = bool
  default     = true
}

variable "api_ingress_cidrs" {
  description = "CIDRs allowed to reach the public API HTTPS listener."
  type        = list(string)
  default     = ["0.0.0.0/0"]
}

variable "xds_ingress_cidrs" {
  description = "CIDRs allowed to reach the public xDS TCP listener. Prefer your operator/dataplane public IP."
  type        = list(string)
}

variable "control_plane_image" {
  description = "Container image URI for the Flowplane release image. Prefer ECR in the same region."
  type        = string
}

variable "cpu" {
  description = "Fargate task CPU units."
  type        = number
  default     = 512
}

variable "memory" {
  description = "Fargate task memory in MiB."
  type        = number
  default     = 1024
}

variable "desired_count" {
  description = "ECS service desired task count."
  type        = number
  default     = 1
}

variable "api_certificate_arn" {
  description = "ACM certificate ARN for the public API ALB listener."
  type        = string
}

variable "oidc_issuer" {
  description = "OIDC issuer URL. Auth0 is the verified provider, but this remains provider-agnostic."
  type        = string
}

variable "oidc_audience" {
  description = "OIDC audience expected by the Flowplane control plane."
  type        = string
}

variable "oidc_jwks_uri" {
  description = "Optional explicit OIDC JWKS URI."
  type        = string
  default     = ""
}

variable "db_name" {
  description = "RDS database name."
  type        = string
  default     = "flowplane"
}

variable "db_username" {
  description = "RDS master username."
  type        = string
  default     = "flowplane"
}

variable "db_instance_class" {
  description = "RDS instance class."
  type        = string
  default     = "db.t4g.micro"
}

variable "db_allocated_storage" {
  description = "RDS allocated storage in GiB."
  type        = number
  default     = 20
}

variable "database_sslmode" {
  description = "sslmode query parameter for FLOWPLANE_DATABASE_URL."
  type        = string
  default     = "require"
}

variable "deletion_protection" {
  description = "Enable deletion protection on stateful resources."
  type        = bool
  default     = false
}

variable "log_retention_days" {
  description = "CloudWatch log retention for the control plane."
  type        = number
  default     = 14
}

variable "secret_encryption_key_secret_arn" {
  description = "Secrets Manager ARN containing the Flowplane secret encryption key as SecretString."
  type        = string
}

variable "secret_kms_key_arns" {
  description = "Optional customer-managed KMS key ARNs used by the Secrets Manager secrets."
  type        = list(string)
  default     = []
}

variable "api_tls_cert_secret_arn" {
  description = "Secrets Manager ARN containing the CP API backend TLS certificate PEM as SecretString."
  type        = string
}

variable "api_tls_key_secret_arn" {
  description = "Secrets Manager ARN containing the CP API backend TLS private key PEM as SecretString."
  type        = string
}

variable "xds_tls_cert_secret_arn" {
  description = "Secrets Manager ARN containing the xDS server certificate PEM as SecretString."
  type        = string
}

variable "xds_tls_key_secret_arn" {
  description = "Secrets Manager ARN containing the xDS server private key PEM as SecretString."
  type        = string
}

variable "xds_tls_client_ca_secret_arn" {
  description = "Secrets Manager ARN containing the dataplane client CA certificate PEM as SecretString."
  type        = string
}

variable "cert_issuer_ca_cert_secret_arn" {
  description = "Secrets Manager ARN containing the dataplane certificate issuer CA certificate PEM as SecretString."
  type        = string
}

variable "cert_issuer_ca_key_secret_arn" {
  description = "Secrets Manager ARN containing the dataplane certificate issuer CA private key PEM as SecretString."
  type        = string
}

variable "cert_issuer_trust_domain" {
  description = "SPIFFE trust domain used for issued dataplane certificates."
  type        = string
  default     = "getflowplane.io"
}
