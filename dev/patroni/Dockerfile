FROM postgres:17
# Patroni 4 с клиентом etcd3 — в отдельном venv: системный Python в Debian запрещает pip install.
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3-venv curl \
 && python3 -m venv /opt/patroni \
 && /opt/patroni/bin/pip install --no-cache-dir "patroni[etcd3]>=4.0,<5" "psycopg[binary]>=3.2,<4" \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /var/lib/postgresql/patroni \
 && chown postgres:postgres /var/lib/postgresql/patroni
COPY patroni/patroni.yml sql/init.sql /etc/patroni/
COPY --chmod=0755 patroni/post_bootstrap.sh /etc/patroni/post_bootstrap.sh
USER postgres
HEALTHCHECK --interval=5s --timeout=3s CMD curl -fs http://localhost:8008/liveness || exit 1
ENTRYPOINT ["/opt/patroni/bin/patroni"]
CMD ["/etc/patroni/patroni.yml"]
