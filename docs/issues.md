# Known Issues & Future Work

## Active Issues

### Multiple Workspaces

**Problem:** Currently the scripts remember the last used workspace in `.lucidos-workspace`, but there's no easy way to switch between workspaces or list available ones.

**Current workaround:** Always specify `-w <path>` when switching workspaces.

**Proposed solutions:**

1. **Named workspaces config file** - Store workspace aliases in `~/.lucidos/workspaces.json`:
   ```json
   {
     "personal": "/Users/me/lucidos-personal",
     "work": "/Users/me/lucidos-work"
   }
   ```
   Then: `./scripts/start.sh -w personal`

2. **Workspace switcher command** - Add `./scripts/switch.sh <name>` that updates `.lucidos-workspace`

3. **Environment-based** - Use `LUCIDOS_WORKSPACE` env var, set per terminal/shell profile

**Decision needed:** Which approach? (1) is most flexible, (3) is simplest.

---

### File Picker Not Yet Implemented

**Problem:** "Import file from documents" asks for path instead of showing file picker.

**Cause:** Frontend is simple Python http.server, no native dialog support.

**Solution:** Implement file picker flow:
1. LLM calls `pick_files` tool
2. Backend returns `action: "show_file_picker"` in response
3. Frontend shows HTML `<input type="file">`
4. User picks files, frontend uploads to `/upload` endpoint
5. Frontend sends chat: "Selected: file1.txt, file2.md"
6. LLM continues with import

**Status:** Planned for implementation.

---

### Populate Script Uses Different Workspace

**Problem:** `populate.sh` uses `./test-workspace` (relative to project), but `start.sh` might use `~/test-workspace`.

**Fix:** Make populate.sh use the saved workspace from `.lucidos-workspace` like other scripts.

---

## Completed

- [x] PDF text extraction for imports
- [x] Binary file support in imports
- [x] Workspace persistence (scripts remember last workspace)
- [x] Current date/time in system prompt
- [x] "Action first" behavior (no clarification loops)
