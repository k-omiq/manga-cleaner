"""Pinned released SDK candidate matrix for cloud providers (Modal & Beam/Beta9).

All versions, hashes, and requirements are derived from official PyPI metadata
and source-code inspection of released wheels as of 2026-09-19.
No fabricated hashes or provisional versions are included.
"""

from dataclasses import dataclass, field
from typing import Dict, List, Optional


@dataclass(frozen=True)
class PackageArtifact:
    filename: str
    sha256: str
    size_bytes: int
    packagetype: str


@dataclass(frozen=True)
class ProviderSDKCandidate:
    provider_id: str
    package_name: str
    pinned_version: str
    release_date: str
    requires_python: str
    pypi_url: str
    doc_url: str
    artifacts: List[PackageArtifact]
    direct_dependencies: List[str]
    source_verified_apis: List[str]
    source_cli_commands: List[str]
    unverified_hosted_behaviors: List[str]
    security_audit_notes: List[str]
    platform_support: Dict[str, str]


# Pinned Modal SDK Candidate
MODAL_CANDIDATE = ProviderSDKCandidate(
    provider_id="modal",
    package_name="modal",
    pinned_version="1.5.5",
    release_date="2026-08-28T19:51:32Z",
    requires_python="<3.15,>=3.10",
    pypi_url="https://pypi.org/pypi/modal/1.5.5/json",
    doc_url="https://modal.com/docs/sdk/py/latest/App",
    artifacts=[
        PackageArtifact(
            filename="modal-1.5.5-py3-none-any.whl",
            sha256="8d10d3ee09818aaba1973b73ce2521ab8961b63a29b5b52e3ff0d25e7a74808e",
            size_bytes=985163,
            packagetype="bdist_wheel",
        ),
        PackageArtifact(
            filename="modal-1.5.5.tar.gz",
            sha256="30df363ed1898cc3d91a09ff3f95c38ab043f6b6294011b01085312c6a0ac777",
            size_bytes=870356,
            packagetype="sdist",
        ),
    ],
    direct_dependencies=[
        "aiohttp",
        "cbor2",
        "certifi",
        "click~=8.1",
        'grpclib<0.4.10,>=0.4.7; python_version < "3.14"',
        'grpclib<0.4.10,>=0.4.9; python_version >= "3.14"',
        "protobuf!=4.24.0,<7.0,>=3.19",
        "rich>=12.0.0",
        "synchronicity~=0.12.5",
        "toml",
        "types-certifi",
        "types-toml",
        "watchfiles",
        "typing_extensions~=4.6",
    ],
    source_verified_apis=[
        "modal.Client.from_credentials(token_id, token_secret) -> _Client (callable in-memory client initialization)",
        "modal.App.deploy(client=client, name=..., environment_name=...) (callable programmatic deploy accepting explicit client)",
        "modal.Workspace.from_context(client=client) -> _Workspace (callable workspace inspection)",
        "workspace.proxy_tokens.create() -> TokenData (callable webhook proxy token creation)",
        "workspace.proxy_tokens.allow(proxy_token_id, environment_name) (callable environment scoping)",
        "workspace.proxy_tokens.revoke(proxy_token_id, environment_name) (callable revocation)",
        "workspace.proxy_tokens.delete(proxy_token_id) (callable token deletion)",
    ],
    source_cli_commands=[
        "modal token set (Click Command wrapper for ~/.modal.toml; MUST NOT be used by provisioner)",
        "modal app deploy (Click Command CLI wrapper)",
        "modal app stop (Click Command CLI wrapper)",
    ],
    unverified_hosted_behaviors=[
        "Actual hosted proxy token authorization enforcement against /mc/v1 endpoints in production control plane",
        "Real deployment URL generation and routing determinism for named persistent apps",
        "Workspace environment isolation limits on non-enterprise tiers",
    ],
    security_audit_notes=[
        "Client.from_credentials allows zero-mutation of global profile (~/.modal.toml); avoids secret leakage to disk",
        "Requires Python >= 3.10; system macOS /usr/bin/python3 (3.9.6) is incompatible; bundled runtime required",
    ],
    platform_support={
        "macos_arm64": "source-compatible pure wheel / execution unverified (blocked on macOS system Python 3.9.6, requires bundled Python >= 3.10)",
        "linux_x86_64": "source-compatible pure wheel / execution unverified (requires runtime verification with Python >= 3.10)",
        "windows_x64": "source-compatible pure wheel / execution unverified (runtime subprocess behavior unverified without Windows native test run)",
    },
)


# Pinned Beta9 SDK Candidate (direct dependency of beam-client)
BETA9_CANDIDATE = ProviderSDKCandidate(
    provider_id="beta9",
    package_name="beta9",
    pinned_version="0.1.268",
    release_date="2026-09-15T17:51:18.988082Z",
    requires_python="<4.0,>=3.8",
    pypi_url="https://pypi.org/pypi/beta9/0.1.268/json",
    doc_url="https://pypi.org/project/beta9/",
    artifacts=[
        PackageArtifact(
            filename="beta9-0.1.268-py3-none-any.whl",
            sha256="eb21e48b9ae0871449b2434b2e735024a59298ffce64953a066d9d36b8358a75",
            size_bytes=318857,
            packagetype="bdist_wheel",
        ),
        PackageArtifact(
            filename="beta9-0.1.268.tar.gz",
            sha256="693e4f65771e40d925946f149d70eedb82baea846de351567a383b7dd11a4b96",
            size_bytes=297790,
            packagetype="sdist",
        ),
    ],
    direct_dependencies=[],
    source_verified_apis=[
        "beta9.client.client.Client(token=..., gateway_host=..., gateway_port=...) (callable client constructor)",
        "beta9.config.SDKSettings(api_token=os.getenv('BEAM_TOKEN')) (settings object respecting env var)",
    ],
    source_cli_commands=[
        "beta9.cli.deployment.delete_deployment (Click Command wrapper for CLI, not direct callable SDK library method)",
        "beta9.cli.deployment.list_deployments (Click Command wrapper for CLI, not direct callable SDK library method)",
        "beta9.cli.token.create_token (Click Command wrapper with --token-type, not direct callable SDK library method)",
        "beta9.cli.token.delete_token (Click Command wrapper for CLI, not direct callable SDK library method)",
        "beta9.cli.config.list_contexts (Click Command wrapper; exposes tokens when --show-token passed)",
        "beta9.cli.config.export_config (Click Command wrapper; exposes raw token in JSON dump)",
    ],
    unverified_hosted_behaviors=[
        "Actual hosted enforcement of 'workspace_restricted' token scope; cannot infer lease or permissions from enum name alone",
    ],
    security_audit_notes=[
        "CLI commands use Click wrappers and are designed for subprocess execution, not direct in-process calling",
        "CLI config commands can dump tokens to stdout if invoked",
    ],
    platform_support={
        "macos_arm64": "source-compatible pure wheel / execution unverified",
        "linux_x86_64": "source-compatible pure wheel / execution unverified",
        "windows_x64": "source-compatible pure wheel / execution unverified without Windows native test run",
    },
)


# Pinned Beam Client Candidate
BEAM_CANDIDATE = ProviderSDKCandidate(
    provider_id="beam",
    package_name="beam-client",
    pinned_version="0.2.211",
    release_date="2026-09-15T18:10:38Z",
    requires_python="<4.0,>=3.8",
    pypi_url="https://pypi.org/pypi/beam-client/0.2.211/json",
    doc_url="https://docs.beam.cloud/v2/getting-started/installation",
    artifacts=[
        PackageArtifact(
            filename="beam_client-0.2.211-py3-none-any.whl",
            sha256="ffa8c9bb086d85ad1bff07bf8ebde65d5e122a4df1469d1aec56598ddf44c561",
            size_bytes=10935,
            packagetype="bdist_wheel",
        ),
        PackageArtifact(
            filename="beam_client-0.2.211.tar.gz",
            sha256="8ca975ce814ada1100d737764024c7f489ac05a72cba3265517ee6452ff7daed",
            size_bytes=7225,
            packagetype="sdist",
        ),
    ],
    direct_dependencies=[
        "packaging>=24.0",
        "requests<3.0.0,>=2.31.0",
        "websockets<16,>=13",
        "beta9<0.2.0,>=0.1.268",
    ],
    source_verified_apis=[
        "beam.client.client.Client(token=...) (callable client constructor inheriting from beta9.client.client.Client)",
    ],
    source_cli_commands=[
        "beam configure (Click Command wrapper; mutates ~/.beam/config.ini directly - MUST NOT be invoked)",
        "beta9.cli.deployment.delete_deployment (Click Command wrapper for CLI, not direct callable SDK library method)",
        "beta9.cli.deployment.list_deployments (Click Command wrapper for CLI, not direct callable SDK library method)",
        "beta9.cli.token.create_token (Click Command wrapper with --token-type, not direct callable SDK library method)",
        "beta9.cli.token.delete_token (Click Command wrapper for CLI, not direct callable SDK library method)",
    ],
    unverified_hosted_behaviors=[
        "Actual hosted enforcement of 'workspace_restricted' token scope; cannot infer lease or permissions from enum name alone",
        "Permission boundary between deploy powers vs submit/status/result/cancel runtime access under hosted tokens",
    ],
    security_audit_notes=[
        "Default 'beam configure' CLI command mutates ~/.beam/config.ini directly - MUST NOT be invoked",
        "Default 'beam config list --show-token' and 'beam config export' expose tokens in stdout/json",
        "BEAM_TOKEN env var allows non-mutating session execution without touching ~/.beam/config.ini",
    ],
    platform_support={
        "macos_arm64": "source-compatible pure wheel / execution unverified (pure wheel on Python 3.8 - 3.12)",
        "linux_x86_64": "source-compatible pure wheel / execution unverified",
        "windows_x64": "unverified / blocked (officially documented ONLY through WSL Ubuntu 22.04; native Windows packaging unverified)",
    },
)

SDK_MATRIX: Dict[str, ProviderSDKCandidate] = {
    "modal": MODAL_CANDIDATE,
    "beam": BEAM_CANDIDATE,
    "beta9": BETA9_CANDIDATE,
}
