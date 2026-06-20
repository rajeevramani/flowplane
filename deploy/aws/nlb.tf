resource "aws_lb" "xds" {
  name               = "${local.name}-xds"
  load_balancer_type = "network"
  internal           = false
  subnets            = values(aws_subnet.public)[*].id
}

resource "aws_lb_target_group" "xds" {
  name        = "${local.name}-xds"
  port        = 18000
  protocol    = "TCP"
  target_type = "ip"
  vpc_id      = aws_vpc.this.id

  # Preserve the real client source IP so the operator-CIDR allowlist on the task SG is
  # meaningful (off by default for IP targets — without this the task sees the NLB node IP).
  preserve_client_ip = true

  health_check {
    enabled             = true
    protocol            = "TCP"
    interval            = 30
    healthy_threshold   = 2
    unhealthy_threshold = 3
  }
}

resource "aws_lb_listener" "xds_tcp" {
  load_balancer_arn = aws_lb.xds.arn
  port              = 18000
  protocol          = "TCP"

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.xds.arn
  }
}

