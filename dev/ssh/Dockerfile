FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssh-server \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /run/sshd \
 && useradd -m -s /bin/bash tester \
 && echo 'tester:tester-secret' | chpasswd \
 && ssh-keygen -A
COPY ssh/sshd_config /etc/ssh/sshd_config
COPY ssh/keys/tester_ed25519.pub ssh/keys/tester_ed25519_pass.pub /tmp/
RUN mkdir -p /home/tester/.ssh && cat /tmp/tester_ed25519.pub /tmp/tester_ed25519_pass.pub > /home/tester/.ssh/authorized_keys \
 && chown -R tester:tester /home/tester/.ssh && chmod 700 /home/tester/.ssh && chmod 600 /home/tester/.ssh/authorized_keys
EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
