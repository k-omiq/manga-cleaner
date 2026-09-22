"""CPU-only local introspection and source-packaging probe.

Validates that trusted cloud templates are syntactically valid, self-contained,
and free of GPU/model weight imports without installing or loading CUDA, torch,
diffusers, or provider SDKs.

LIMITATION DISCLOSURE:
Static AST analysis detects explicit import nodes (both top-level and nested).
It does NOT prove absence of dynamic import side-effects (e.g. __import__,
importlib.import_module, or native extension initialization during execution).
"""

import ast
from datetime import datetime, timezone
import hashlib
import json
import logging
import os
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Set

FORBIDDEN_MODULES = {
    "torch",
    "torchvision",
    "torchaudio",
    "diffusers",
    "transformers",
    "accelerate",
    "cuda",
    "triton",
    "tensorrt",
    "onnxruntime",
    "cupy",
}

PROHIBITED_WEIGHT_EXTENSIONS = {
    ".bin",
    ".safetensors",
    ".pt",
    ".pth",
    ".ckpt",
    ".onnx",
    ".engine",
    ".weights",
    ".whl",
}

MAX_TEMPLATE_FILE_SIZE_BYTES = 100 * 1024  # 100 KiB safety limit
REQUIRED_TEMPLATES = {"modal/app.py", "beam/app.py"}

AST_STATIC_ANALYSIS_DISCLOSURE = (
    "Static AST analysis inspects syntax trees for import statements and structure. "
    "It does NOT prove zero runtime side effects if dynamic execution (__import__, importlib) occurs."
)


@dataclass
class TemplateProbeResult:
    path: str
    valid_syntax: bool
    all_imports: List[str]
    prohibited_imports_found: List[str]
    has_health_endpoint: bool
    gpu_stack_imported_locally: bool
    sha256: str
    size_bytes: int
    errors: List[str] = field(default_factory=list)


@dataclass
class StagingVerificationResult:
    template_dir: str
    templates_found: List[str]
    missing_required_templates: List[str]
    prohibited_files_found: List[str]
    all_templates_valid: bool
    probes: Dict[str, TemplateProbeResult] = field(default_factory=dict)
    errors: List[str] = field(default_factory=list)
    limitations_noted: str = AST_STATIC_ANALYSIS_DISCLOSURE


def inspect_template_source(source_path: Path) -> TemplateProbeResult:
    """Inspect a Python template file using AST parsing without executing it or importing GPU stacks."""
    path_str = str(source_path.resolve())
    errors = []

    if not source_path.exists():
        return TemplateProbeResult(
            path=path_str,
            valid_syntax=False,
            all_imports=[],
            prohibited_imports_found=[],
            has_health_endpoint=False,
            gpu_stack_imported_locally=False,
            sha256="",
            size_bytes=0,
            errors=[f"File not found: {path_str}"],
        )

    # Size rejection BEFORE reading full content into memory
    stat = source_path.stat()
    size_bytes = stat.st_size
    if size_bytes > MAX_TEMPLATE_FILE_SIZE_BYTES:
        return TemplateProbeResult(
            path=path_str,
            valid_syntax=False,
            all_imports=[],
            prohibited_imports_found=[],
            has_health_endpoint=False,
            gpu_stack_imported_locally=False,
            sha256="",
            size_bytes=size_bytes,
            errors=[
                f"File size {size_bytes} exceeds maximum threshold {MAX_TEMPLATE_FILE_SIZE_BYTES} bytes. "
                "Content read rejected before allocation."
            ],
        )

    raw_bytes = source_path.read_bytes()
    sha256 = hashlib.sha256(raw_bytes).hexdigest()

    try:
        tree = ast.parse(raw_bytes, filename=path_str)
        valid_syntax = True
    except SyntaxError as e:
        return TemplateProbeResult(
            path=path_str,
            valid_syntax=False,
            all_imports=[],
            prohibited_imports_found=[],
            has_health_endpoint=False,
            gpu_stack_imported_locally=False,
            sha256=sha256,
            size_bytes=size_bytes,
            errors=[f"Syntax error in template: {e}"],
        )

    all_imports: List[str] = []
    prohibited_imports: List[str] = []
    has_health = False

    # Walk the entire AST to detect both top-level and nested imports
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                root_pkg = alias.name.split(".")[0]
                all_imports.append(alias.name)
                if root_pkg in FORBIDDEN_MODULES:
                    prohibited_imports.append(alias.name)
        elif isinstance(node, ast.ImportFrom):
            if node.module:
                root_pkg = node.module.split(".")[0]
                all_imports.append(node.module)
                if root_pkg in FORBIDDEN_MODULES:
                    prohibited_imports.append(node.module)
        elif isinstance(node, ast.Call):
            target_mod = None
            if isinstance(node.func, ast.Name) and node.func.id == "__import__":
                if node.args and isinstance(node.args[0], ast.Constant) and isinstance(node.args[0].value, str):
                    target_mod = node.args[0].value
            elif isinstance(node.func, ast.Attribute) and node.func.attr == "import_module":
                if node.args and isinstance(node.args[0], ast.Constant) and isinstance(node.args[0].value, str):
                    target_mod = node.args[0].value
            if target_mod:
                root_pkg = target_mod.split(".")[0]
                all_imports.append(target_mod)
                if root_pkg in FORBIDDEN_MODULES:
                    prohibited_imports.append(target_mod)
        elif isinstance(node, ast.FunctionDef):
            if "health" in node.name.lower():
                has_health = True

    if prohibited_imports:
        errors.append(
            f"Forbidden GPU/heavy modules imported (top-level or nested): {prohibited_imports}"
        )

    # Invariant: verify that no GPU stack has been imported into sys.modules
    gpu_imported = any(mod in sys.modules for mod in FORBIDDEN_MODULES)
    if gpu_imported:
        errors.append("Invariant violated: GPU modules present in sys.modules")

    return TemplateProbeResult(
        path=path_str,
        valid_syntax=valid_syntax and len(errors) == 0,
        all_imports=all_imports,
        prohibited_imports_found=prohibited_imports,
        has_health_endpoint=has_health,
        gpu_stack_imported_locally=gpu_imported,
        sha256=sha256,
        size_bytes=size_bytes,
        errors=errors,
    )


def verify_template_staging(template_dir: Path) -> StagingVerificationResult:
    """Verify that trusted template sources are available in staging without weight artifacts."""
    template_dir = template_dir.resolve()
    errors: List[str] = []
    templates_found: List[str] = []
    prohibited_files: List[str] = []
    probes: Dict[str, TemplateProbeResult] = {}

    if not template_dir.exists():
        return StagingVerificationResult(
            template_dir=str(template_dir),
            templates_found=[],
            missing_required_templates=list(REQUIRED_TEMPLATES),
            prohibited_files_found=[],
            all_templates_valid=False,
            probes={},
            errors=[f"Template staging directory does not exist: {template_dir}"],
        )

    for root, _, files in os.walk(template_dir):
        for f in files:
            full_path = Path(root) / f
            rel_path = str(full_path.relative_to(template_dir))
            suffix = full_path.suffix.lower()

            if suffix in PROHIBITED_WEIGHT_EXTENSIONS:
                prohibited_files.append(rel_path)
                errors.append(f"Prohibited weight/binary artifact found in staging: {rel_path}")

            if f.endswith(".py"):
                templates_found.append(rel_path)
                probe = inspect_template_source(full_path)
                probes[rel_path] = probe
                if not probe.valid_syntax or probe.errors:
                    errors.extend(probe.errors)

    # Enforce that all required provider templates are present
    missing_required = [req for req in REQUIRED_TEMPLATES if req not in templates_found]
    if missing_required:
        errors.append(f"Missing required provider templates in staging: {missing_required}")

    all_valid = (
        len(errors) == 0
        and len(missing_required) == 0
        and len(prohibited_files) == 0
        and all(p.valid_syntax and len(p.errors) == 0 for p in probes.values())
    )

    return StagingVerificationResult(
        template_dir=str(template_dir),
        templates_found=templates_found,
        missing_required_templates=missing_required,
        prohibited_files_found=prohibited_files,
        all_templates_valid=all_valid,
        probes=probes,
        errors=errors,
    )


def run_probe_and_log(
    template_dir: Optional[Path] = None,
    log_file: Optional[Path] = None,
) -> StagingVerificationResult:
    """Run staging verification and output structured log with real UTC timestamp."""
    if template_dir is None:
        template_dir = Path(__file__).parent / "templates"

    if log_file is None:
        log_file = Path("/tmp/manga-cloud-p1-fixed-probe.log")

    result = verify_template_staging(template_dir)

    log_entry = {
        "event": "p1_offline_template_probe",
        "timestamp_utc": datetime.now(timezone.utc).isoformat(),
        "template_dir": result.template_dir,
        "templates_found": result.templates_found,
        "missing_required_templates": result.missing_required_templates,
        "prohibited_files": result.prohibited_files_found,
        "all_valid": result.all_templates_valid,
        "limitations": result.limitations_noted,
        "probes": {k: asdict(v) for k, v in result.probes.items()},
        "errors": result.errors,
    }

    log_file.parent.mkdir(parents=True, exist_ok=True)
    with open(log_file, "w", encoding="utf-8") as f:
        f.write(json.dumps(log_entry, indent=2))

    return result


if __name__ == "__main__":
    result = run_probe_and_log()
    print(f"Staging Probe Passed: {result.all_templates_valid}")
    for path, probe in result.probes.items():
        print(f" - {path}: valid={probe.valid_syntax}, sha256={probe.sha256[:16]}...")
    if result.errors:
        print("Errors:", result.errors)
        sys.exit(1)
    sys.exit(0)
