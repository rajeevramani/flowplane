resource "aws_lb" "api" {
  name               = "${local.name}-api"
  load_balancer_type = "application"
  internal           = false
  security_groups    = [aws_security_group.alb.id]
  subnets            = values(aws_subnet.public)[*].id
}

resource "aws_lb_target_group" "api" {
  name        = "${local.name}-api"
  port        = 8080
  protocol    = "HTTPS"
  target_type = "ip"
  vpc_id      = aws_vpc.this.id

  health_check {
    enabled             = true
    path                = "/healthz"
    protocol            = "HTTPS"
    matcher             = "200"
    interval            = 30
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
  }
}

resource "aws_lb_listener" "api_https" {
  load_balancer_arn = aws_lb.api.arn
  port              = 443
  protocol          = "HTTPS"
  certificate_arn   = var.api_certificate_arn
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.api.arn
  }
}

