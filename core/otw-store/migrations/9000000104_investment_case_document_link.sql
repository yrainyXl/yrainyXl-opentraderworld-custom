-- Investment Case ↔ Editor research document becomes an OPTIONAL association.
--
-- The case is the business entity on its own (decision/change records, thesis
-- and current state stay authoritative and independent of the page); the Editor
-- page is only the research body the case currently points to. Deleting that
-- page from the Editor must NOT destroy the case: the link is a reference, not
-- a required existence. This changes:
--
--   1) investment_cases.document_id → nullable
--   2) FK action RESTRICT → ON DELETE SET NULL (deleting a linked page detaches
--      the case instead of failing with a 409 dead end)
--   3) one-time fold: case research pages still sitting at the tree root move
--      under a root folder titled “股票研究” (folder is created if missing;
--      an existing same-named root folder is reused). Pages the user already
--      filed elsewhere are left alone.
--
-- The “股票研究” title is duplicated by the Rust constant
-- RESEARCH_FOLDER_TITLE in core/otw-store/src/investment_cases.rs — keep the
-- two in sync if the folder name ever changes.

ALTER TABLE investment_cases ALTER COLUMN document_id DROP NOT NULL;

ALTER TABLE investment_cases
    DROP CONSTRAINT IF EXISTS investment_cases_document_id_fkey;

ALTER TABLE investment_cases
    ADD CONSTRAINT investment_cases_document_id_fkey
    FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE SET NULL;

-- Ensure the research root folder exists (reuse an existing one when present).
INSERT INTO documents (id, parent_id, kind, title, position)
SELECT gen_random_uuid(), NULL, 'folder', '股票研究',
       COALESCE((SELECT MAX(position) + 1 FROM documents
                 WHERE parent_id IS NULL AND kind = 'folder'), 1)
WHERE NOT EXISTS (
    SELECT 1 FROM documents
    WHERE kind = 'folder' AND title = '股票研究' AND parent_id IS NULL
);

-- Move case-linked research pages that are still at the root into the folder.
UPDATE documents d
SET parent_id = f.id,
    updated_at = now()
FROM (SELECT id FROM documents
      WHERE kind = 'folder' AND title = '股票研究' AND parent_id IS NULL
      LIMIT 1) f
WHERE d.parent_id IS NULL
  AND d.kind = 'page'
  AND EXISTS (SELECT 1 FROM investment_cases ic WHERE ic.document_id = d.id);
