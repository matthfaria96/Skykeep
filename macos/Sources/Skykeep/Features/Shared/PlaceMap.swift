import MapKit
import SwiftUI

/// A place shown on a map inside Skykeep — an address, an event's location, where a photo was
/// taken — instead of handing it to the Maps app.
struct PlaceMap: View {
    var query: String? = nil
    var coordinate: CLLocationCoordinate2D? = nil
    var title: String = ""

    @State private var found: CLLocationCoordinate2D?
    @State private var name = ""
    @State private var failed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let c = found ?? coordinate {
                Map(initialPosition: .region(MKCoordinateRegion(center: c, latitudinalMeters: 1200, longitudinalMeters: 1200))) {
                    Marker(name.isEmpty ? title : name, coordinate: c)
                }
                .mapControls { MapZoomStepper(); MapCompass() }
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            } else if failed {
                ContentUnavailableView("Lugar Não Encontrado", systemImage: "mappin.slash", description: Text(query ?? ""))
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            if let query { Text(query).font(.callout).foregroundStyle(.secondary).textSelection(.enabled) }
        }
        .padding(12)
        .frame(width: 380, height: 320)
        .task {
            guard coordinate == nil, let query, !query.isEmpty else { return }
            let req = MKLocalSearch.Request()
            req.naturalLanguageQuery = query
            if let item = try? await MKLocalSearch(request: req).start().mapItems.first {
                found = item.location.coordinate
                name = item.name ?? ""
            } else {
                failed = true
            }
        }
    }
}
