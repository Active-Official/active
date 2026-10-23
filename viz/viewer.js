// Static viewer for `active viz` JSON exports. No evaluation, no
// network, no external dependencies: the export is already redacted.
(() => {
	const out = document.getElementById("out");

	function table(headers, rows) {
		const el = document.createElement("table");
		const head = document.createElement("tr");
		for (const h of headers) {
			const th = document.createElement("th");
			th.textContent = h;
			head.appendChild(th);
		}
		el.appendChild(head);
		for (const row of rows) {
			const tr = document.createElement("tr");
			for (const cell of row) {
				const td = document.createElement("td");
				td.textContent = cell;
				tr.appendChild(td);
			}
			el.appendChild(tr);
		}
		return el;
	}

	function render(doc) {
		out.textContent = "";
		if (doc.usage && doc.limits) {
			out.appendChild(
				table(
					["Budget", "Used", "Limit", "Headroom"],
					Object.keys(doc.limits).map((k) => [
						k,
						String(doc.usage[k] ?? "-"),
						String(doc.limits[k] ?? "-"),
						String(doc.headroom?.[k] ?? "-"),
					]),
				),
			);
			if (doc.warnings?.length) {
				const p = document.createElement("p");
				p.className = "tainted";
				p.textContent = "Warnings: " + doc.warnings.join(", ");
				out.appendChild(p);
			}
			return;
		}
		if (doc.nodes && doc.edges) {
			out.appendChild(
				table(
					["Node", "Kind", "Secret"],
					doc.nodes.map((n) => [
						n.label ?? n.id,
						n.kind ?? "",
						n.secret ? "yes" : "",
					]),
				),
			);
			out.appendChild(
				table(
					["Edge from", "Edge to"],
					doc.edges.map((e) => [e.from, e.to]),
				),
			);
			return;
		}
		if (doc.fields) {
			out.appendChild(
				table(
					["Path", "Type", "Required", "Constraints"],
					doc.fields.map((f) => [
						f.path ?? "",
						f.type ?? "",
						f.required ? "yes" : "",
						(f.constraints ?? []).join(", "),
					]),
				),
			);
			return;
		}
		if (doc.steps) {
			out.appendChild(
				table(
					["Step", "Path", "Kind", "Codes", "Tainted"],
					doc.steps.map((s, i) => [
						String(i),
						s.path ?? "(internal)",
						s.kind ?? "",
						(s.codes ?? []).join(","),
						s.tainted ? "yes" : "",
					]),
				),
			);
			return;
		}
		const pre = document.createElement("pre");
		pre.textContent = JSON.stringify(doc, null, 2);
		out.appendChild(pre);
	}

	document.getElementById("file").addEventListener("change", (event) => {
		const file = event.target.files?.[0];
		if (!file) return;
		const reader = new FileReader();
		reader.onload = () => {
			try {
				render(JSON.parse(String(reader.result)));
			} catch (e) {
				out.textContent = "Invalid JSON: " + e.message;
			}
		};
		reader.readAsText(file);
	});
})();
