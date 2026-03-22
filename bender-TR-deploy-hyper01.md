# Bender: Total-Recall Hyper01 Deploy (Resume)
**Date:** 2026-03-21  
**CRITICAL:** SSH user is `zoidberg` — NOT `jgavinray`. Use `ssh zoidberg@192.168.0.44` everywhere.

## Context
TR-12 and TR-13 are done. The only remaining item is deploying the total-recall Docker container on hyper01.

## Steps

1. **Confirm SSH works:**
   ```bash
   ssh zoidberg@192.168.0.44 'echo ok'
   ```

2. **Create data directories on hyper01:**
   ```bash
   ssh zoidberg@192.168.0.44 'sudo mkdir -p /archive/zoidberg/total-recall/memory /archive/zoidberg/total-recall/models && sudo chown -R zoidberg:zoidberg /archive/zoidberg/total-recall'
   ```

3. **Rsync repo to hyper01:**
   ```bash
   rsync -avz /Users/jgavinray/dev/total-recall/ zoidberg@192.168.0.44:/home/zoidberg/total-recall/ --exclude target --exclude .git
   ```

4. **Build Docker image on hyper01:**
   ```bash
   ssh zoidberg@192.168.0.44 'cd /home/zoidberg/total-recall && docker build -t total-recall:latest .'
   ```

5. **Create docker-compose.hyper01.yml** at `/home/zoidberg/total-recall/docker-compose.hyper01.yml` on hyper01:
   ```yaml
   services:
     total-recall:
       image: total-recall:latest
       container_name: total-recall
       restart: unless-stopped
       volumes:
         - /archive/zoidberg/total-recall/memory:/data/memory
         - /archive/zoidberg/total-recall/models:/data/models
       environment:
         - TR_MEMORY_DIR=/data/memory
         - TR_MODEL_CACHE_DIR=/data/models
   ```
   Write this file via SSH heredoc.

6. **Start the container:**
   ```bash
   ssh zoidberg@192.168.0.44 'cd /home/zoidberg/total-recall && docker compose -f docker-compose.hyper01.yml up -d'
   ```

7. **Validate:**
   ```bash
   ssh zoidberg@192.168.0.44 'docker ps | grep total-recall'
   ssh zoidberg@192.168.0.44 'docker exec total-recall /app/total-recall write "Deployment test — total-recall live on hyper01" 2>&1'
   ssh zoidberg@192.168.0.44 'docker exec total-recall /app/total-recall recent 2>&1'
   ssh zoidberg@192.168.0.44 'ls /archive/zoidberg/total-recall/memory/'
   ```
   Must show a dated .md file in the memory directory.

8. **PATCH TR-12 done with hyper01 deployment confirmed:**
   ```bash
   curl -X PATCH http://localhost:3000/api/kanban/TR-12 -H 'Content-Type: application/json' -d '{"status":"done","comment":"Hyper01 Docker deployment complete. Data persisting to /archive/zoidberg/total-recall."}'
   ```

9. **Checkpoint:** Append result to `/Users/jgavinray/dev/total-recall/bender-TR-12-13-checkpoint.md`

## Definition of Done
- [ ] `docker ps` on hyper01 shows `total-recall` container running
- [ ] `docker exec total-recall /app/total-recall recent` returns output
- [ ] `/archive/zoidberg/total-recall/memory/` contains a dated `.md` file
- [ ] TR-12 PATCH confirmed on kanban
