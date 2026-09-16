export function verificationStatusKey(backup: { metadata_valid: boolean; hash_verified: boolean }):
  "backupInvalid" | "backupVerified" | "backupNotVerified" {
  if (!backup.metadata_valid) return "backupInvalid";
  return backup.hash_verified ? "backupVerified" : "backupNotVerified";
}
