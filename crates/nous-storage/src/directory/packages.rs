use super::*;
use sqlx::Row;
use std::collections::BTreeSet;
impl ReadView {
    pub async fn package_reference_addresses(
        &mut self,
        package: &SubjectPackage,
    ) -> Result<Vec<ReferenceBinding>> {
        let rows=sqlx::query("SELECT address,target FROM reference_addresses WHERE namespace_id=?1 UNION SELECT r.address,r.target FROM imported_reference_addresses r JOIN bundle_installations b ON b.id=r.installation_id WHERE b.namespace_id=?1 AND b.subject_id=?2 AND b.uninstalled_at IS NULL")
            .bind(package.subject.namespace_id.as_str()).bind(package.subject.id.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let allowed = package.addressable_targets();
        let mut bindings = BTreeSet::new();
        for row in rows {
            let target: SemanticTarget = serde_json::from_str(row.get("target"))?;
            if allowed.contains(&target) {
                bindings.insert(ReferenceBinding {
                    address: row.get::<String, _>("address").parse()?,
                    target,
                });
            }
        }
        Ok(bindings.into_iter().collect())
    }
}
impl WriteTransaction {
    pub async fn import_reference_address(
        &mut self,
        installation: &str,
        binding: &ReferenceBinding,
    ) -> Result<()> {
        binding.validate()?;
        sqlx::query("INSERT INTO imported_reference_addresses(installation_id,address,target) VALUES(?,?,?)")
            .bind(installation).bind(binding.address.as_str()).bind(serde_json::to_string(&binding.target)?)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
