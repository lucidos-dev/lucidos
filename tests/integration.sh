#!/bin/bash
set -e

echo "Building Docker image..."
docker build -t lucidos-test .

echo "Starting container..."
docker run -d --name lucidos-test-container -p 3000:3000 lucidos-test

echo "Waiting for API..."
sleep 10

echo "Testing health endpoint..."
curl -f http://localhost:3000/health

echo ""
echo "Testing chat endpoint..."
curl -X POST http://localhost:3000/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello"}'

echo ""
echo "Testing artifacts endpoint..."
curl http://localhost:3000/artifacts

echo ""
echo "Cleaning up..."
docker stop lucidos-test-container
docker rm lucidos-test-container

echo "All tests passed!"
