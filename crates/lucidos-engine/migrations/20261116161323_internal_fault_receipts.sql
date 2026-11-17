-- Retry receipts for the fault inbox. InternalFaultRecorded remains deduplicated
-- by kind; every consumed inbox identity is remembered across report boundaries.
CREATE TABLE internal_fault_receipts (
    id UUID PRIMARY KEY
);
