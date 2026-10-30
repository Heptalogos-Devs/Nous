"""Public bundle operations share archive production with the host facade."""

from collections.abc import Mapping

from pydantic import JsonValue

from ..amemorix.contracts import AMemorixOperation, AMemorixRequest
from ..contracts import InstallBundle
from ..operations import encoded, object_value
from ..source_scope import SourceReadScope
from ..types import JsonObject
from .management import ManagementClient
from .requests import operation_identity, operation_values, request_scope


class BundleClient(ManagementClient):
    async def bundles(self, request: Mapping[str, JsonValue]) -> JsonObject:
        values = operation_values(request)
        source_read = values.pop("source_read", None)
        if source_read is not None:
            return await self.with_source_read(SourceReadScope.model_validate(source_read)).bundles(
                {"action": request.get("action"), "request": values}
            )
        namespace, subject = await request_scope(self, values)
        action = request.get("action")
        if not isinstance(action, str):
            raise ValueError("Bundle action must be text")
        if action == "install" and "bundle" in values:
            values["operation_id"] = operation_identity(values)
            return await self.install_bundle(
                namespace, subject, InstallBundle.model_validate(values)
            )
        if action == "preview":
            action = "export"
            values["preview"] = True
        elif action == "install":
            action = "import"
        if action in {"get", "uninstall"} and "id" in values:
            values["installation_id"] = values.pop("id")
        operation = AMemorixRequest(
            namespace_id=namespace,
            subject_id=subject,
            operation=AMemorixOperation(
                tool="memory_bundle_admin", arguments={"action": action, **values}
            ),
        )
        return object_value(
            await self.request("POST", "/api/integrations/amemorix", body=encoded(operation))
        )
