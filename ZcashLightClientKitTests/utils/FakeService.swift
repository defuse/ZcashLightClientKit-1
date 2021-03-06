//
//  FakeService.swift
//  ZcashLightClientKitTests
//
//  Created by Francisco Gindre on 10/23/19.
//  Copyright © 2019 Electric Coin Company. All rights reserved.
//

import Foundation
import SwiftProtobuf
@testable import ZcashLightClientKit

struct LightWalletServiceMockResponse: LightWalletServiceResponse {
    var errorCode: Int32
    var errorMessage: String
    var unknownFields: UnknownStorage
}

class MockLightWalletService: LightWalletService {
    
    private var service = LightWalletGRPCService(channel: ChannelProvider().channel())
    private var latestHeight: BlockHeight
    
    init(latestBlockHeight: BlockHeight) {
        self.latestHeight = latestBlockHeight
    }
    func latestBlockHeight(result: @escaping (Result<BlockHeight, LightWalletServiceError>) -> Void) {
        DispatchQueue.global().asyncAfter(deadline: .now() + 1) {
            result(.success(self.latestHeight))
        }
    }
    
    func latestBlockHeight() throws -> BlockHeight {
        return self.latestHeight
    }
    
    func blockRange(_ range: Range<BlockHeight>, result: @escaping (Result<[ZcashCompactBlock], LightWalletServiceError>) -> Void) {
        self.service.blockRange(range, result: result)
    }
    
    func blockRange(_ range: CompactBlockRange) throws -> [ZcashCompactBlock] {
        try self.service.blockRange(range)
    }
    
    func submit(spendTransaction: Data, result: @escaping (Result<LightWalletServiceResponse, LightWalletServiceError>) -> Void) {
        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 1) {
            result(.success(LightWalletServiceMockResponse(errorCode: 0, errorMessage: "", unknownFields: UnknownStorage())))
        }
    }
    
    func submit(spendTransaction: Data) throws -> LightWalletServiceResponse {
        return LightWalletServiceMockResponse(errorCode: 0, errorMessage: "", unknownFields: UnknownStorage())
    }
}
