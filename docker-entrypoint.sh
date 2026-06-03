#!/bin/bash
set -e

PGDATA="/workspace/data/postgres"
PGUSER="lucidos"
PGDATABASE="lucidos"

# Initialize PostgreSQL data directory if needed
if [ ! -f "$PGDATA/PG_VERSION" ]; then
    echo "Initializing PostgreSQL database..."

    # Create postgres user if it doesn't exist
    if ! id -u postgres >/dev/null 2>&1; then
        useradd -r -s /bin/false postgres
    fi

    # Initialize the database
    chown -R postgres:postgres "$PGDATA"
    sudo -u postgres /usr/lib/postgresql/17/bin/initdb -D "$PGDATA" --auth-local=trust --auth-host=trust

    # Configure for local connections only
    echo "listen_addresses = 'localhost'" >> "$PGDATA/postgresql.conf"
    echo "unix_socket_directories = '/var/run/postgresql'" >> "$PGDATA/postgresql.conf"

    # Create socket directory
    mkdir -p /var/run/postgresql
    chown postgres:postgres /var/run/postgresql
fi

# Ensure correct ownership
chown -R postgres:postgres "$PGDATA"
mkdir -p /var/run/postgresql
chown postgres:postgres /var/run/postgresql

# Start PostgreSQL
echo "Starting PostgreSQL..."
sudo -u postgres /usr/lib/postgresql/17/bin/pg_ctl -D "$PGDATA" -l "$PGDATA/postgresql.log" start

# Wait for PostgreSQL to be ready
echo "Waiting for PostgreSQL..."
for i in {1..30}; do
    if sudo -u postgres /usr/lib/postgresql/17/bin/pg_isready -q; then
        break
    fi
    sleep 1
done

# Create database and user if they don't exist
if ! sudo -u postgres psql -lqt | cut -d \| -f 1 | grep -qw "$PGDATABASE"; then
    echo "Creating database..."
    sudo -u postgres createdb "$PGDATABASE"
fi

# Enable pgvector extension
echo "Enabling pgvector extension..."
sudo -u postgres psql -d "$PGDATABASE" -c "CREATE EXTENSION IF NOT EXISTS vector;" 2>/dev/null || true

echo "PostgreSQL ready"

# Set DATABASE_URL for the engine
export DATABASE_URL="postgres://$PGUSER@localhost/$PGDATABASE"

# Start Lucidos engine
echo "Starting Lucidos engine..."
exec /usr/bin/lucidos-engine
