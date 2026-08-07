import { Hero } from "@/sections/landing/Hero";
import { WhyRusty } from "@/sections/landing/WhyRusty";
import { HowItWorks } from "@/sections/landing/HowItWorks";
import { FeatureGrid } from "@/sections/landing/FeatureGrid";
import { ComponentsTable } from "@/sections/landing/ComponentsTable";
import { Comparison } from "@/sections/landing/Comparison";
import { Limitations } from "@/sections/landing/Limitations";
import { FinalCta } from "@/sections/landing/FinalCta";

export default function LandingPage() {
  return (
    <main>
      <Hero />
      <WhyRusty />
      <HowItWorks />
      <FeatureGrid />
      <ComponentsTable />
      <Comparison />
      <Limitations />
      <FinalCta />
    </main>
  );
}
