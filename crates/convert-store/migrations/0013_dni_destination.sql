-- Preserve the published swap/fallback number while separating call delivery.
ALTER TABLE dni_pools ADD COLUMN destination_phone text;
UPDATE dni_pools SET destination_phone=fallback_phone;
ALTER TABLE dni_pools ALTER COLUMN destination_phone SET NOT NULL;
ALTER TABLE dni_pools ADD CONSTRAINT dni_destination_phone_format
    CHECK (destination_phone ~ '^\+1[0-9]{10}$');

-- An older application image may be restored after a failed cutover. Its
-- INSERT does not know this column, so retain the old fallback=destination rule.
CREATE FUNCTION default_dni_destination() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.destination_phone IS NULL THEN
        NEW.destination_phone := NEW.fallback_phone;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER dni_destination_legacy_insert
    BEFORE INSERT ON dni_pools FOR EACH ROW EXECUTE FUNCTION default_dni_destination();
