output "api_alb_dns_name" {
  description = "DNS name for the public API ALB. Point the API hostname at this value."
  value       = aws_lb.api.dns_name
}

output "api_alb_zone_id" {
  description = "Hosted zone ID for the public API ALB."
  value       = aws_lb.api.zone_id
}

output "xds_nlb_dns_name" {
  description = "DNS name for the public xDS NLB. Point the xDS hostname at this value with DNS-only routing."
  value       = aws_lb.xds.dns_name
}

output "xds_nlb_zone_id" {
  description = "Hosted zone ID for the public xDS NLB."
  value       = aws_lb.xds.zone_id
}

output "ecs_cluster_name" {
  description = "ECS cluster name."
  value       = aws_ecs_cluster.this.name
}

output "ecs_service_name" {
  description = "ECS service name."
  value       = aws_ecs_service.control_plane.name
}

output "cloudwatch_log_group" {
  description = "CloudWatch log group containing control-plane logs."
  value       = aws_cloudwatch_log_group.control_plane.name
}

output "rds_endpoint" {
  description = "RDS endpoint hostname."
  value       = aws_db_instance.this.address
}

