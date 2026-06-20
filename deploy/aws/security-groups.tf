resource "aws_security_group" "alb" {
  name        = "${local.name}-api-alb"
  description = "Public API ALB"
  vpc_id      = aws_vpc.this.id
}

resource "aws_vpc_security_group_ingress_rule" "alb_https" {
  for_each = toset(var.api_ingress_cidrs)

  security_group_id = aws_security_group.alb.id
  description       = "HTTPS API"
  from_port         = 443
  to_port           = 443
  ip_protocol       = "tcp"
  cidr_ipv4         = each.value
}

resource "aws_security_group" "ecs_tasks" {
  name        = "${local.name}-ecs-tasks"
  description = "Flowplane control-plane ECS tasks"
  vpc_id      = aws_vpc.this.id
}

resource "aws_vpc_security_group_egress_rule" "alb_to_ecs_api" {
  security_group_id            = aws_security_group.alb.id
  description                  = "HTTPS to ECS API target"
  from_port                    = 8080
  to_port                      = 8080
  ip_protocol                  = "tcp"
  referenced_security_group_id = aws_security_group.ecs_tasks.id
}

resource "aws_vpc_security_group_ingress_rule" "ecs_api_from_alb" {
  security_group_id            = aws_security_group.ecs_tasks.id
  description                  = "HTTPS API from ALB"
  from_port                    = 8080
  to_port                      = 8080
  ip_protocol                  = "tcp"
  referenced_security_group_id = aws_security_group.alb.id
}

resource "aws_vpc_security_group_ingress_rule" "ecs_xds" {
  for_each = toset(var.xds_ingress_cidrs)

  security_group_id = aws_security_group.ecs_tasks.id
  description       = "xDS mTLS from approved dataplane/operator CIDRs through NLB"
  from_port         = 18000
  to_port           = 18000
  ip_protocol       = "tcp"
  cidr_ipv4         = each.value
}

resource "aws_vpc_security_group_egress_rule" "ecs_all" {
  security_group_id = aws_security_group.ecs_tasks.id
  ip_protocol       = "-1"
  cidr_ipv4         = "0.0.0.0/0"
}

# NLB health checks for the xDS target originate from the NLB nodes inside the VPC, not from
# the operator CIDRs. Without this rule the TCP health check is dropped and the target is marked
# unhealthy, so the NLB has no target to forward to (xDS connections time out).
resource "aws_vpc_security_group_ingress_rule" "ecs_xds_healthcheck" {
  security_group_id = aws_security_group.ecs_tasks.id
  description       = "xDS NLB health checks from within the VPC"
  from_port         = 18000
  to_port           = 18000
  ip_protocol       = "tcp"
  cidr_ipv4         = var.vpc_cidr
}

resource "aws_security_group" "rds" {
  name        = "${local.name}-rds"
  description = "Flowplane RDS PostgreSQL"
  vpc_id      = aws_vpc.this.id
}

resource "aws_vpc_security_group_ingress_rule" "rds_from_ecs" {
  security_group_id            = aws_security_group.rds.id
  description                  = "PostgreSQL from ECS tasks"
  from_port                    = 5432
  to_port                      = 5432
  ip_protocol                  = "tcp"
  referenced_security_group_id = aws_security_group.ecs_tasks.id
}

resource "aws_vpc_security_group_egress_rule" "rds_all" {
  security_group_id = aws_security_group.rds.id
  ip_protocol       = "-1"
  cidr_ipv4         = "0.0.0.0/0"
}
