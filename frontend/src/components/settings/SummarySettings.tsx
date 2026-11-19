"use client";

import { SummaryModelSettings } from "@/components/SummaryModelSettings";
import { SummaryTemplateSettings } from "@/components/SummaryTemplateSettings";

// The Summary settings page. Starts as the previous composition.
export function SummarySettings() {
  return (
    <div className="space-y-6">
      <SummaryModelSettings />
      <SummaryTemplateSettings />
    </div>
  );
}
